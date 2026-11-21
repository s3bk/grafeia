//! The parts of an sfnt container the PDF exporter needs.
//!
//! The exporter does not render from these tables — the layout already did
//! that. It reads them to embed the font: the head/hhea/maxp/hmtx numbers go
//! into the font descriptor and the `/W` array, and the `CFF ` table is what
//! gets subsetted and put in the PDF.

pub struct Sfnt<'a> {
    data: &'a [u8],
    tables: Vec<(String, usize, usize)>,
}

fn slice(d: &[u8], pos: usize, len: usize) -> Result<&[u8], String> {
    d.get(pos..pos + len)
        .ok_or_else(|| format!("font table truncated at {} (+{})", pos, len))
}

fn be16(d: &[u8], pos: usize) -> Result<u16, String> {
    let b = slice(d, pos, 2)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

fn be32(d: &[u8], pos: usize) -> Result<u32, String> {
    let b = slice(d, pos, 4)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be16s(d: &[u8], pos: usize) -> Result<i16, String> {
    Ok(be16(d, pos)? as i16)
}

impl<'a> Sfnt<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Sfnt<'a>, String> {
        if data.len() < 12 {
            return Err("font file is too short to be an sfnt".into());
        }
        let num_tables = be16(data, 4)? as usize;
        let mut tables = Vec::with_capacity(num_tables);
        for i in 0..num_tables {
            let rec = 12 + 16 * i;
            let tag = slice(data, rec, 4)?;
            let tag = String::from_utf8_lossy(tag).into_owned();
            let offset = be32(data, rec + 8)? as usize;
            let len = be32(data, rec + 12)? as usize;
            if offset + len > data.len() {
                return Err(format!("table {} runs past the end of the file", tag));
            }
            tables.push((tag, offset, len));
        }
        Ok(Sfnt { data, tables })
    }

    pub fn table(&self, tag: &str) -> Option<&'a [u8]> {
        self.tables
            .iter()
            .find(|(t, _, _)| t == tag)
            .map(|&(_, off, len)| &self.data[off..off + len])
    }

    fn require(&self, tag: &str) -> Result<&'a [u8], String> {
        self.table(tag)
            .ok_or_else(|| format!("font has no '{}' table", tag))
    }

    pub fn units_per_em(&self) -> Result<u16, String> {
        be16(self.require("head")?, 18)
    }

    pub fn num_glyphs(&self) -> Result<u16, String> {
        be16(self.require("maxp")?, 4)
    }

    /// The advance width of every glyph, in font units. Glyphs past
    /// `numberOfHMetrics` repeat the last entry, as the hmtx format says.
    pub fn advances(&self) -> Result<Vec<u16>, String> {
        let hhea = self.require("hhea")?;
        let n_hmetrics = be16(hhea, 34)? as usize;
        let num_glyphs = self.num_glyphs()? as usize;
        let hmtx = self.require("hmtx")?;
        let mut advances = Vec::with_capacity(num_glyphs);
        let mut last = 0;
        for gid in 0..num_glyphs {
            if gid < n_hmetrics {
                last = be16(hmtx, gid * 4)?;
            }
            advances.push(last);
        }
        Ok(advances)
    }

    pub fn ascender(&self) -> Result<i16, String> {
        be16s(self.require("hhea")?, 4)
    }

    pub fn descender(&self) -> Result<i16, String> {
        be16s(self.require("hhea")?, 6)
    }

    /// Cap height from OS/2 when the table is new enough to carry it.
    pub fn cap_height(&self) -> Option<i16> {
        let os2 = self.table("OS/2")?;
        let version = be16(os2, 0).ok()?;
        if version < 2 {
            return None;
        }
        let cap = be16s(os2, 88).ok()?;
        (cap != 0).then_some(cap)
    }

    /// PostScript name (name id 6). Falls back to the full name when a font
    /// only carries that.
    pub fn postscript_name(&self) -> Option<String> {
        let name = self.table("name")?;
        let count = be16(name, 2).ok()? as usize;
        let strings = be16(name, 4).ok()? as usize;
        let mut fallback: Option<String> = None;
        for i in 0..count {
            let rec = 6 + 12 * i;
            let platform = be16(name, rec).ok()?;
            let language = be16(name, rec + 4).ok()?;
            let id = be16(name, rec + 6).ok()?;
            let len = be16(name, rec + 8).ok()? as usize;
            let off = be16(name, rec + 10).ok()? as usize;
            let bytes = name.get(strings + off..strings + off + len)?;
            let text = if platform == 3 || platform == 0 {
                // UTF-16BE; the PostScript name is ASCII in practice.
                let words: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&words)
            } else {
                String::from_utf8_lossy(bytes).into_owned()
            };
            match id {
                6 => return Some(sanitize(&text)),
                4 if platform == 3 && language == 0x409 => fallback = Some(sanitize(&text)),
                _ => {}
            }
        }
        fallback
    }
}

/// A PDF name may not contain whitespace or delimiters; a PostScript name
/// with a space in it (some fonts have one) becomes a name that still reads.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_+.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}
