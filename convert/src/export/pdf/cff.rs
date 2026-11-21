//! Subsetting a CFF font down to the glyphs a document actually uses.
//!
//! The embedded program is a **CID-keyed** CFF: the PDF refers to glyphs by
//! CID through `/Identity-H`, and `CIDToGIDMap /Identity` makes the CID the
//! glyph index in the program. Writing a CID-keyed font means the charstring
//! order *is* the CID order, so the exporter is free to choose which glyphs to
//! keep and in what order, and no `CIDToGIDMap` table has to be written.
//!
//! What is kept verbatim: the charstrings themselves, the global and local
//! subroutines, and the private dicts. Subroutine calls inside a charstring are
//! indices into those, so copying the subroutines whole is what makes verbatim
//! charstrings legal. What is rebuilt: the charset (now the identity), the
//! FDSelect (remapped to the Font DICTs that are still used), and the Top DICT,
//! whose offsets all move.
//!
//! Outlines are not rewritten, so the program keeps the font's own units per
//! em and its own FontMatrix; the PDF's `/W` widths are in 1/1000 em, which is
//! what `/W` means regardless of the program's units.

use std::collections::{BTreeSet, HashMap};

use super::sfnt::Sfnt;

/// One subset font, ready to be wrapped in a CIDFontType0 / FontFile3 pair.
pub struct Subset {
    /// The font program: a CID-keyed CFF, to be embedded as
    /// `/FontFile3` with `/Subtype /CIDFontType0C`.
    pub cff: Vec<u8>,
    pub name: String,
    pub num_glyphs: u16,
    /// Original glyph id → CID in the subset.
    pub gid_map: HashMap<u16, u16>,
    /// Advance per CID, in 1/1000 em: the `/W` array.
    pub widths: Vec<f32>,
    /// Font bounding box in 1/1000 em.
    pub bbox: [f32; 4],
    pub ascent: f32,
    pub descent: f32,
    pub cap_height: f32,
    pub italic_angle: f32,
    /// How many of the font's glyphs the document used.
    pub used: usize,
}

pub fn subset(font: &[u8], used: &BTreeSet<u16>) -> Result<Subset, String> {
    let sfnt = Sfnt::parse(font)?;
    let upem = sfnt.units_per_em()?;
    if upem == 0 {
        return Err("font claims 0 units per em".into());
    }
    let cff = sfnt.table("CFF ").ok_or_else(|| {
        if sfnt.table("CFF2").is_some() {
            "the font's outlines are CFF2, which is not supported".to_string()
        } else if sfnt.table("glyf").is_some() {
            "the font's outlines are TrueType (glyf), which is not supported yet".to_string()
        } else {
            "the font has no CFF table".to_string()
        }
    })?;
    let advances = sfnt.advances()?;
    let source = Source::parse(cff)?;

    // .notdef first, then the used glyphs in order. Sorting keeps the output
    // deterministic, which makes two runs comparable.
    let mut keep: Vec<u16> = Vec::with_capacity(used.len() + 1);
    keep.push(0);
    for &gid in used {
        if gid != 0 && (gid as usize) < source.charstrings.len() {
            keep.push(gid);
        }
    }

    let mut gid_map = HashMap::with_capacity(keep.len());
    for (new, &old) in keep.iter().enumerate() {
        gid_map.insert(old, new as u16);
    }

    // Font DICTs still in use, in the order they are first needed.
    let mut fd_index: HashMap<u8, u8> = HashMap::new();
    let mut fd_order: Vec<u8> = Vec::new();
    let mut glyph_fd: Vec<u8> = Vec::with_capacity(keep.len());
    for &old in &keep {
        let fd = *source.glyph_fd.get(old as usize).unwrap_or(&0);
        let next = fd_order.len() as u8;
        let new = *fd_index.entry(fd).or_insert_with(|| {
            fd_order.push(fd);
            next
        });
        glyph_fd.push(new);
    }
    let privates: Vec<&Private> = fd_order
        .iter()
        .map(|&fd| {
            source
                .privates
                .get(fd as usize)
                .ok_or_else(|| format!("glyph refers to missing Font DICT {}", fd))
        })
        .collect::<Result<_, _>>()?;

    let charstrings: Vec<&[u8]> = keep
        .iter()
        .map(|&gid| {
            let (a, b) = source.charstrings[gid as usize];
            &source.data[a..b]
        })
        .collect();

    // The CFF's FontMatrix maps charstring units into em fractions; keep the
    // font's own value, exactly (not the decimal the font rounded it to).
    let matrix = 1.0f32 / upem as f32;
    let cff = assemble(
        &source.name,
        &charstrings,
        &glyph_fd,
        &privates,
        matrix,
        source.bbox,
        source.italic_angle,
        source.gsubrs,
        charstrings.len() as u16,
    );

    let scale = 1000.0 / upem as f32;
    let widths: Vec<f32> = keep
        .iter()
        .map(|&gid| advances.get(gid as usize).copied().unwrap_or(0) as f32 * scale)
        .collect();
    let bbox = [
        source.bbox[0] as f32 * scale,
        source.bbox[1] as f32 * scale,
        source.bbox[2] as f32 * scale,
        source.bbox[3] as f32 * scale,
    ];
    let ascent = sfnt.ascender().unwrap_or(0) as f32 * scale;
    let descent = sfnt.descender().unwrap_or(0) as f32 * scale;
    let cap_height = sfnt
        .cap_height()
        .map(|c| c as f32 * scale)
        .unwrap_or(bbox[3] * 0.7);

    Ok(Subset {
        name: source.name,
        num_glyphs: charstrings.len() as u16,
        gid_map,
        widths,
        bbox,
        ascent,
        descent,
        cap_height,
        italic_angle: source.italic_angle.unwrap_or(0.0) as f32,
        used: used.len(),
        cff,
    })
}

// ---------------------------------------------------------------- reading

fn u8_at(d: &[u8], p: usize) -> Result<u8, String> {
    d.get(p)
        .copied()
        .ok_or_else(|| format!("CFF truncated at {}", p))
}

fn u16_at(d: &[u8], p: usize) -> Result<u16, String> {
    let b = d.get(p..p + 2).ok_or_else(|| format!("CFF truncated at {}", p))?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

/// An INDEX: the byte range of each item, and the offset just past the whole
/// index.
struct Index {
    items: Vec<(usize, usize)>,
    end: usize,
}

fn index(d: &[u8], pos: usize) -> Result<Index, String> {
    let count = u16_at(d, pos)? as usize;
    if count == 0 {
        return Ok(Index {
            items: Vec::new(),
            end: pos + 2,
        });
    }
    let off_size = u8_at(d, pos + 2)? as usize;
    if off_size == 0 || off_size > 4 {
        return Err(format!("CFF INDEX with offSize {}", off_size));
    }
    let base = pos + 3;
    let mut offsets = Vec::with_capacity(count + 1);
    for i in 0..=count {
        let mut v = 0usize;
        for k in 0..off_size {
            v = (v << 8) | u8_at(d, base + i * off_size + k)? as usize;
        }
        offsets.push(v);
    }
    let data_start = base + (count + 1) * off_size;
    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let start = data_start + offsets[i].wrapping_sub(1);
        let end = data_start + offsets[i + 1].wrapping_sub(1);
        if start >= end || end > d.len() {
            return Err(format!("CFF INDEX item {} is out of bounds", i));
        }
        items.push((start, end));
    }
    Ok(Index {
        items,
        end: data_start + offsets[count].wrapping_sub(1),
    })
}

/// One DICT entry: the operator, its operands, and where the operands sit in
/// the original bytes.
struct Entry {
    op: u16,
    operands: Vec<f64>,
}

fn dict(d: &[u8], (start, end): (usize, usize)) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    let mut operands: Vec<f64> = Vec::new();
    let mut i = start;
    while i < end {
        let b0 = u8_at(d, i)?;
        match b0 {
            0..=21 => {
                let op = if b0 == 12 {
                    let op = 1200 + u8_at(d, i + 1)? as u16;
                    i += 2;
                    op
                } else {
                    i += 1;
                    b0 as u16
                };
                out.push(Entry {
                    op,
                    operands: std::mem::take(&mut operands),
                });
            }
            28 => {
                let v = u16_at(d, i + 1)? as i16 as f64;
                operands.push(v);
                i += 3;
            }
            29 => {
                let b = d.get(i + 1..i + 5).ok_or("CFF DICT truncated")?;
                operands.push(i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64);
                i += 5;
            }
            30 => {
                // real: nibbles until 0xf
                let mut s = String::new();
                i += 1;
                'outer: loop {
                    let byte = u8_at(d, i)?;
                    i += 1;
                    for nib in [byte >> 4, byte & 0x0f] {
                        match nib {
                            0..=9 => s.push((b'0' + nib) as char),
                            0xa => s.push('.'),
                            0xb => s.push('E'),
                            0xc => s.push_str("E-"),
                            0xd => {}
                            0xe => s.push('-'),
                            _ => break 'outer,
                        }
                    }
                }
                operands.push(s.parse().unwrap_or(0.0));
            }
            32..=246 => {
                operands.push(b0 as f64 - 139.0);
                i += 1;
            }
            247..=250 => {
                let v = (b0 as f64 - 247.0) * 256.0 + u8_at(d, i + 1)? as f64 + 108.0;
                operands.push(v);
                i += 2;
            }
            251..=254 => {
                let v = -(b0 as f64 - 251.0) * 256.0 - u8_at(d, i + 1)? as f64 - 108.0;
                operands.push(v);
                i += 2;
            }
            _ => return Err(format!("CFF DICT has a reserved operand byte {}", b0)),
        }
    }
    Ok(out)
}

fn op_int(entries: &[Entry], op: u16) -> Option<i64> {
    entries
        .iter()
        .find(|e| e.op == op)
        .and_then(|e| e.operands.first())
        .map(|v| *v as i64)
}

fn op_pair(entries: &[Entry], op: u16) -> Option<(i64, i64)> {
    entries.iter().find(|e| e.op == op).and_then(|e| {
        if e.operands.len() >= 2 {
            Some((e.operands[0] as i64, e.operands[1] as i64))
        } else {
            None
        }
    })
}

/// A private dict with its local subroutines still attached: the subrs offset
/// is relative to the dict's own start, so the two only work as one blob.
struct Private {
    dict_size: usize,
    blob: Vec<u8>,
}

fn private_at(d: &[u8], (size, offset): (i64, i64)) -> Result<Private, String> {
    if size <= 0 || offset < 0 {
        return Err("CFF private dict has a bad size or offset".into());
    }
    let start = offset as usize;
    let end = start + size as usize;
    if end > d.len() {
        return Err("CFF private dict runs past the end of the table".into());
    }
    // Local subrs, if any, sit right after the dict; the offset in the dict is
    // relative to the dict start, so copying dict + subrs verbatim keeps it.
    let entries = dict(d, (start, end))?;
    let mut blob_end = end;
    if let Some(subrs) = op_int(&entries, 19) {
        let pos = start as i64 + subrs;
        if pos >= 0 && (pos as usize) < d.len() {
            blob_end = index(d, pos as usize)?.end.max(end);
        }
    }
    Ok(Private {
        dict_size: size as usize,
        blob: d[start..blob_end].to_vec(),
    })
}

struct Source<'a> {
    data: &'a [u8],
    charstrings: Vec<(usize, usize)>,
    gsubrs: &'a [u8],
    privates: Vec<Private>,
    glyph_fd: Vec<u8>,
    bbox: [i64; 4],
    italic_angle: Option<f64>,
    name: String,
}

impl<'a> Source<'a> {
    fn parse(d: &'a [u8]) -> Result<Source<'a>, String> {
        let hdr_size = u8_at(d, 2)? as usize;
        let names = index(d, hdr_size)?;
        let name = names
            .items
            .first()
            .map(|&(a, b)| String::from_utf8_lossy(&d[a..b]).into_owned())
            .unwrap_or_else(|| "Subset".to_string());
        let tops = index(d, names.end)?;
        let top_entry = *tops
            .items
            .first()
            .ok_or("CFF has no Top DICT")?;
        let strings = index(d, tops.end)?;
        let gsubrs_index = index(d, strings.end)?;
        let top = dict(d, top_entry)?;

        let charstrings_off = op_int(&top, 17).ok_or("CFF Top DICT has no CharStrings")?;
        let charstrings = index(d, charstrings_off as usize)?;
        if charstrings.items.is_empty() {
            return Err("CFF has no charstrings".into());
        }

        let bbox = top
            .iter()
            .find(|e| e.op == 5)
            .filter(|e| e.operands.len() >= 4)
            .map(|e| {
                [
                    e.operands[0] as i64,
                    e.operands[1] as i64,
                    e.operands[2] as i64,
                    e.operands[3] as i64,
                ]
            })
            .unwrap_or([-1000, -300, 2000, 1000]);
        let italic_angle = top
            .iter()
            .find(|e| e.op == 1202)
            .and_then(|e| e.operands.first().copied());
        if let Some(entries) = top.iter().find(|e| e.op == 1206) {
            if entries.operands.first().copied().unwrap_or(2.0) as i64 != 2 {
                return Err("CFF uses Type 1 charstrings, which are not supported".into());
            }
        }

        let is_cid = top.iter().any(|e| e.op == 1230);
        let (privates, glyph_fd) = if is_cid {
            let fdarray_off = op_int(&top, 1236).ok_or("CID-keyed CFF without an FDArray")?;
            let fdselect_off = op_int(&top, 1237).ok_or("CID-keyed CFF without an FDSelect")?;
            let fdarray = index(d, fdarray_off as usize)?;
            let mut privates = Vec::with_capacity(fdarray.items.len());
            for &item in &fdarray.items {
                let font_dict = dict(d, item)?;
                let private = op_pair(&font_dict, 18).ok_or("Font DICT without a Private entry")?;
                privates.push(private_at(d, private)?);
            }
            let fd = fd_select(d, fdselect_off as usize, charstrings.items.len())?;
            (privates, fd)
        } else {
            let private = op_pair(&top, 18).ok_or("CFF Top DICT without a Private entry")?;
            (
                vec![private_at(d, private)?],
                vec![0u8; charstrings.items.len()],
            )
        };

        Ok(Source {
            data: d,
            charstrings: charstrings.items,
            // The whole Global Subr INDEX, offsets and all: it is copied
            // verbatim, so its items keep the indices charstrings call.
            gsubrs: &d[strings.end..gsubrs_index.end],
            privates,
            glyph_fd,
            bbox,
            italic_angle,
            name,
        })
    }
}

fn fd_select(d: &[u8], pos: usize, num_glyphs: usize) -> Result<Vec<u8>, String> {
    let format = u8_at(d, pos)?;
    let mut out = vec![0u8; num_glyphs];
    match format {
        0 => {
            for (gid, slot) in out.iter_mut().enumerate() {
                *slot = u8_at(d, pos + 1 + gid)?;
            }
        }
        3 => {
            let n_ranges = u16_at(d, pos + 1)? as usize;
            let mut ranges = Vec::with_capacity(n_ranges);
            for i in 0..n_ranges {
                let p = pos + 3 + i * 3;
                ranges.push((u16_at(d, p)? as usize, u8_at(d, p + 2)?));
            }
            let sentinel = u16_at(d, pos + 3 + n_ranges * 3)? as usize;
            for i in 0..n_ranges {
                let (start, fd) = ranges[i];
                let end = if i + 1 < n_ranges {
                    ranges[i + 1].0
                } else {
                    sentinel
                };
                for gid in start..end.min(num_glyphs) {
                    out[gid] = fd;
                }
            }
        }
        _ => return Err(format!("CFF FDSelect format {} is not supported", format)),
    }
    Ok(out)
}

// ---------------------------------------------------------------- writing

fn off_size(max: usize) -> usize {
    if max < 0x100 {
        1
    } else if max < 0x10000 {
        2
    } else if max < 0x100_0000 {
        3
    } else {
        4
    }
}

fn write_index(items: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(items.len() as u16).to_be_bytes());
    if items.is_empty() {
        return out;
    }
    let total: usize = items.iter().map(|i| i.len()).sum();
    let n = off_size(total + 1);
    out.push(n as u8);
    let mut offset = 1usize;
    for item in items {
        for k in (0..n).rev() {
            out.push(((offset >> (8 * k)) & 0xff) as u8);
        }
        offset += item.len();
    }
    for k in (0..n).rev() {
        out.push(((offset >> (8 * k)) & 0xff) as u8);
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// Integers are always written in the five-byte form so that a DICT's length
/// does not depend on the values of its offsets, which is what lets the
/// offsets be computed in one pass over a placeholder.
fn put_int(d: &mut Vec<u8>, v: i64) {
    d.push(29);
    d.extend_from_slice(&(v as i32).to_be_bytes());
}

fn put_op(d: &mut Vec<u8>, op: u16) {
    if op >= 1200 {
        d.push(12);
        d.push((op - 1200) as u8);
    } else {
        d.push(op as u8);
    }
}

fn put_real(d: &mut Vec<u8>, v: f64) {
    let mut s = format!("{:.12}", v);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    let mut nibbles: Vec<u8> = Vec::with_capacity(s.len() + 1);
    for c in s.chars() {
        nibbles.push(match c {
            '0'..='9' => c as u8 - b'0',
            '.' => 0xa,
            'E' => 0xb,
            '-' => 0xe,
            _ => continue,
        });
    }
    nibbles.push(0xf);
    if nibbles.len() % 2 == 1 {
        nibbles.push(0xf);
    }
    d.push(30);
    for pair in nibbles.chunks(2) {
        d.push((pair[0] << 4) | pair[1]);
    }
}

fn build_top(
    charset: i64,
    fdselect: i64,
    fdarray: i64,
    charstrings: i64,
    matrix: f32,
    bbox: [i64; 4],
    italic_angle: Option<f64>,
    num_cids: u16,
) -> Vec<u8> {
    let mut d = Vec::new();
    // ROS: registry "Adobe", ordering "Identity" (the first two strings of our
    // own string index, which start at SID 391), supplement 0.
    put_int(&mut d, 391);
    put_int(&mut d, 392);
    put_int(&mut d, 0);
    put_op(&mut d, 1230);
    put_int(&mut d, num_cids as i64);
    put_op(&mut d, 1234);
    // [s 0 0 s 0 0]: scale only, no shear, no translation.
    for v in [matrix as f64, 0.0, 0.0, matrix as f64, 0.0, 0.0] {
        put_real(&mut d, v);
    }
    put_op(&mut d, 1207);
    for v in bbox {
        put_int(&mut d, v);
    }
    put_op(&mut d, 5);
    if let Some(angle) = italic_angle {
        put_real(&mut d, angle);
        put_op(&mut d, 1202);
    }
    put_int(&mut d, charset);
    put_op(&mut d, 15);
    put_int(&mut d, fdselect);
    put_op(&mut d, 1237);
    put_int(&mut d, fdarray);
    put_op(&mut d, 1236);
    put_int(&mut d, charstrings);
    put_op(&mut d, 17);
    d
}

fn build_fdarray(privates: &[&Private], offsets: &[i64]) -> Vec<u8> {
    let dicts: Vec<Vec<u8>> = privates
        .iter()
        .zip(offsets.iter())
        .map(|(p, &off)| {
            let mut d = Vec::new();
            put_int(&mut d, p.dict_size as i64);
            put_int(&mut d, off);
            put_op(&mut d, 18);
            d
        })
        .collect();
    let refs: Vec<&[u8]> = dicts.iter().map(|d| d.as_slice()).collect();
    write_index(&refs)
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    name: &str,
    charstrings: &[&[u8]],
    glyph_fd: &[u8],
    privates: &[&Private],
    matrix: f32,
    bbox: [i64; 4],
    italic_angle: Option<f64>,
    gsubrs: &[u8],
    num_cids: u16,
) -> Vec<u8> {
    let name_index = write_index(&[name.as_bytes()]);
    // Two strings: "Adobe" and "Identity", which the ROS entry names by SID.
    let strings = write_index(&[b"Adobe".as_ref(), b"Identity".as_ref()]);
    let charstrings_index = write_index(charstrings);

    // Charset format 0 with the identity: glyph i has CID i.
    let mut charset = Vec::with_capacity(1 + 2 * charstrings.len().saturating_sub(1));
    charset.push(0u8);
    for gid in 1..charstrings.len() {
        charset.extend_from_slice(&(gid as u16).to_be_bytes());
    }

    // FDSelect format 0: one byte per glyph.
    let mut fdselect = Vec::with_capacity(1 + glyph_fd.len());
    fdselect.push(0u8);
    fdselect.extend_from_slice(glyph_fd);

    // The DICTs are written twice: once with placeholder offsets to learn how
    // long they are (their length does not depend on the values), then with
    // the real ones.
    let placeholders = vec![0i64; privates.len()];
    let fdarray_len = build_fdarray(privates, &placeholders).len();
    let top_len = build_top(0, 0, 0, 0, matrix, bbox, italic_angle, num_cids).len();
    let top_index_len = write_index(&[&build_top(0, 0, 0, 0, matrix, bbox, italic_angle, num_cids)])
        .len();

    // header: major, minor, hdrSize, offSize; then the name index
    let mut pos = 4 + name_index.len();
    let top_off = pos;
    pos += top_index_len;
    pos += strings.len();
    pos += gsubrs.len();
    let charstrings_off = pos;
    pos += charstrings_index.len();
    let charset_off = pos;
    pos += charset.len();
    let fdselect_off = pos;
    pos += fdselect.len();
    let fdarray_off = pos;
    pos += fdarray_len;
    let mut private_offs = Vec::with_capacity(privates.len());
    for p in privates {
        private_offs.push(pos as i64);
        pos += p.blob.len();
    }
    let _ = top_off;

    let top = build_top(
        charset_off as i64,
        fdselect_off as i64,
        fdarray_off as i64,
        charstrings_off as i64,
        matrix,
        bbox,
        italic_angle,
        num_cids,
    );
    debug_assert_eq!(top.len(), top_len);
    let top_index = write_index(&[&top]);
    debug_assert_eq!(top_index.len(), top_index_len);
    let fdarray = build_fdarray(privates, &private_offs);
    debug_assert_eq!(fdarray.len(), fdarray_len);

    let mut out = Vec::with_capacity(pos);
    out.extend_from_slice(&[1u8, 0, 4, 1]);
    out.extend_from_slice(&name_index);
    out.extend_from_slice(&top_index);
    out.extend_from_slice(&strings);
    out.extend_from_slice(gsubrs);
    out.extend_from_slice(&charstrings_index);
    out.extend_from_slice(&charset);
    out.extend_from_slice(&fdselect);
    out.extend_from_slice(&fdarray);
    for p in privates {
        out.extend_from_slice(&p.blob);
    }
    out
}
