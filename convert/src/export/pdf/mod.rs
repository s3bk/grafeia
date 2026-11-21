//! The PDF exporter: a laid-out document becomes a PDF whose text is text.
//!
//! The old exporter drew every word as a form XObject full of outlines, so a
//! fourteen-page article came to six megabytes and nothing in it could be
//! selected or searched. This one subsets each font down to the glyphs the
//! document actually uses, embeds the subset as a CID-keyed CFF, and writes
//! the words with `Tf`/`TJ` at the positions the layout chose. Diagrams —
//! objects that draw themselves into a scene — still become filled paths,
//! because they are not text.
//!
//! The entry point is [`write`], which takes a laid-out [`State`] and returns
//! the bytes of a PDF; [`write_file`] puts them on disk.

pub mod cff;
pub mod sfnt;


use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use grafeia_core::draw::{Cache, Layout, RenderItem, WordPart};
use grafeia_core::layout::Columns;
use grafeia_core::text::grapheme_indices;
use grafeia_core::*;

use pathfinder_content::outline::{ContourIterFlags, Outline};
use pathfinder_content::segment::SegmentKind;
use pathfinder_geometry::{rect::RectF, vector::Vector2F};
use pathfinder_renderer::scene::{DrawPathId, Scene};

use pdf::build::{CatalogBuilder, PageBuilder, PdfBuilder};
use pdf::content::{Color, Matrix, Op, Point, Rgb, TextDrawAdjusted, Winding};
use pdf::encoding::{BaseEncoding, Encoding};
use pdf::file::FileOptions;
use pdf::font::{
    CidToGidMap, FontDescriptor, FontStream3, FontType, FontTypeExt, Type0Font, CIDFont,
};
use pdf::font::{Font as PdfFont, FontData};
use pdf::object::{Lazy, MaybeRef, RcRef, Rectangle, Ref, Resolve, Resources, Stream, Updater};
use pdf::file::Trailer;
use pdf::primitive::{Dictionary, Name, PdfString, Primitive};

/// PDF user space is points — 72 to the inch — and the layout is millimetres.
const PT_PER_MM: f32 = 72.0 / 25.4;

/// The 50% grey of a margin number.
const MARGIN_GREY: (u8, u8, u8) = (128, 128, 128);

/// How the PDF is written.
#[derive(Clone, Debug)]
pub struct Options {
    /// Subset every font to the glyphs the document uses, so a face costs only
    /// the letters that are on the page.
    pub subset_fonts: bool,

    /// The complete source the document was made from. When it is given, it
    /// travels inside the PDF, so the file can be opened again later and edited
    /// with the very text and style it was set from.
    pub source: Option<Source>,
}

impl Default for Options {
    fn default() -> Self {
        Options { subset_fonts: true, source: None }
    }
}

/// Where a document came from: the markdown, and the style it was laid out
/// with. The names are the file names to write the parts back out under.
#[derive(Clone, Debug)]
pub struct Source {
    pub markdown_name: String,
    pub markdown: String,
    pub style_name: String,
    pub style: String,
}

/// The key the source travels under, in the first page's dictionary.
const SOURCE_KEY: &str = "Grafeia";

/// Write a laid-out document as a PDF.
pub fn write(state: &State, options: &Options) -> Result<Vec<u8>, String> {
    let subset_fonts = options.subset_fonts;
    let mut cache = Cache::new();
    let _ = cache.layout(&state.storage, &state.design, &state.target, state.root);
    let columns = cache.columns.as_ref().ok_or("the layout produced no columns")?;

    let (plans, index) = plan_fonts(&state, &cache, columns, subset_fonts)?;

    let mut builder = PdfBuilder::new(FileOptions::cached());
    // The pdf crate numbers the first object it creates 0, and object 0 is the
    // free-list head in a PDF file, not a referenceable object: anything that
    // pointed at it would point nowhere. Burn one object so that every object
    // this writer creates has a real number.
    builder
        .create(Dictionary::new())
        .map_err(|e| format!("object zero: {}", e))?;
    let mut resources = Resources::default();
    for plan in &plans {
        let font = build_font(&mut builder, plan)?;
        let rref = builder.create(font).map_err(|e| e.to_string())?;
        resources
            .fonts
            .insert(plan.name.clone(), Lazy::from(rref));
    }

    let media: RectF = state.target.media_box.into();
    let origin = media.origin();
    let page_height = media.height() * PT_PER_MM;
    let page_width = media.width() * PT_PER_MM;
    let media_box = Rectangle {
        left: 0.0,
        bottom: 0.0,
        right: page_width,
        top: page_height,
    };
    let trim: RectF = state.target.trim_box.into();
    let trim_box = Rectangle {
        left: (trim.origin().x() - origin.x()) * PT_PER_MM,
        right: (trim.origin().x() - origin.x() + trim.width()) * PT_PER_MM,
        top: page_height - (trim.origin().y() - origin.y()) * PT_PER_MM,
        bottom: page_height - (trim.origin().y() - origin.y() + trim.height()) * PT_PER_MM,
    };

    let mut pages = Vec::new();
    for column in columns.columns() {
        let mut content = Content::new(page_height);
        let content_box: RectF = state.target.content_box.into();
        for (y, line) in column {
            for (x, size, item, tag) in line {
                let at = content_box.origin() + Vector2F::new(x.value, y.value);
                match item {
                    RenderItem::Word(key, part, font) => {
                        if let Some(layout) = cache.word_layout_cache.get(&(font, key, part)) {
                            let plan = &plans[index[&font.font_face]];
                            content.text(plan, layout, font.size.value, at, false);
                        }
                    }
                    RenderItem::Margin(key, font, hang) => {
                        if let Some(layout) =
                            cache.word_layout_cache.get(&(font, key, WordPart::Full))
                        {
                            let plan = &plans[index[&font.font_face]];
                            let at = at - Vector2F::new(hang.value, 0.0);
                            content.text(plan, layout, font.size.value, at, true);
                        }
                    }
                    RenderItem::Symbol(key, font) => {
                        if let Some(layout) = cache.symbol_layout_cache.get(&(font, key)) {
                            let plan = &plans[index[&font.font_face]];
                            content.text(plan, layout, font.size.value, at, false);
                        }
                    }
                    RenderItem::Object(key) => {
                        content.object(&state, key, tag, size.into(), at);
                    }
                    RenderItem::Empty => {}
                }
            }
        }
        let mut page = PageBuilder::default();
        page.ops = content.ops;
        page.media_box = Some(media_box);
        page.trim_box = Some(trim_box);
        page.resources = resources.clone();
        pages.push(page);
    }

    // The source travels inside the PDF: the markdown and the style file in
    // full, in a stream of their own under a key of our own in the first
    // page's dictionary. That is what lets the file be opened again later and
    // edited with the very text and style it was set from. It is a stream and
    // not a dictionary of strings so that the writer compresses it, and so
    // that a megabyte of article cannot become two megabytes of hex.
    if let Some(source) = &options.source {
        if let Some(page) = pages.first_mut() {
            let payload = encode_source(source);
            let stream = Stream::new((), &payload).map_err(|e| e.to_string())?;
            let stream = builder.create(stream).map_err(|e| e.to_string())?;
            let mut grafeia = Dictionary::new();
            grafeia.insert(
                Name::from("Format"),
                Primitive::String(PdfString::from(SOURCE_FORMAT)),
            );
            // `RcRef<Stream>` is a reference to the object it created; that is
            // what goes in the dictionary.
            grafeia.insert(Name::from("Source"), Primitive::from(stream));
            page.other
                .insert(Name::from(SOURCE_KEY), Primitive::Dictionary(grafeia));
        }
    }

    let catalog = CatalogBuilder::from_pages(pages)
        .build(&mut builder)
        .map_err(|e| e.to_string())?;

    let mut trailer = Trailer {
        size: 0,
        prev_trailer_pos: None,
        root: builder.create(catalog).map_err(|e| e.to_string())?,
        encrypt_dict: None,
        info_dict: builder.info.take(),
        id: Vec::new(),
    };
    builder
        .storage
        .save(&mut trailer)
        .map_err(|e| e.to_string())?;
    let data: Vec<u8> = builder.storage.into_inner();

    info!("{} pages, {} bytes", columns_len(state), data.len());
    Ok(data)
}

/// Write a laid-out document as a PDF at `path`.
pub fn write_file(state: &State, path: impl AsRef<Path>, options: &Options) -> Result<(), String> {
    let path = path.as_ref();
    let data = write(state, options)?;
    std::fs::write(path, &data).map_err(|e| format!("{}: {}", path.display(), e))?;
    Ok(())
}

/// The source embedded in a PDF, if it carries one.
///
/// This is the other half of [`Options::source`]: the markdown and the style
/// come back out under the names they went in with, so the file that was set
/// from them is also the file that can be edited and set again.
pub fn extract(path: impl AsRef<Path>) -> Result<Source, String> {
    let path = path.as_ref();
    let file = FileOptions::cached()
        .open(path)
        .map_err(|e| format!("{}: {}", path.display(), e))?;
    let page = file
        .get_page(0)
        .map_err(|e| format!("{}: {}", path.display(), e))?;
    let what = path.display();
    let dict = match page.other.get(SOURCE_KEY) {
        Some(Primitive::Dictionary(dict)) => dict,
        _ => {
            return Err(format!(
                "{}: no grafeia source in it (written with --no-source?)",
                what
            ))
        }
    };
    let reference = match dict.get("Source") {
        Some(Primitive::Reference(reference)) => *reference,
        _ => return Err(format!("{}: the source has no stream", what)),
    };
    let resolver = file.resolver();
    let stream: RcRef<Stream<()>> = resolver
        .get(Ref::new(reference))
        .map_err(|e| format!("{}: {}", what, e))?;
    let data = stream
        .data(&resolver)
        .map_err(|e| format!("{}: {}", what, e))?;
    decode_source(&data, &what.to_string())
}

/// The first line of the source stream: what the rest of the bytes mean.
const SOURCE_FORMAT: &str = "grafeia-source 1";

/// The source as it travels inside the PDF: one line of magic, then each part
/// as `kind name-length body-length`, the name, and the body.
fn encode_source(source: &Source) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(SOURCE_FORMAT.as_bytes());
    out.push(b'\n');
    for (kind, name, body) in [
        ("markdown", &source.markdown_name, &source.markdown),
        ("style", &source.style_name, &source.style),
    ] {
        out.extend(format!("{} {} {}\n", kind, name.len(), body.len()).into_bytes());
        out.extend(name.as_bytes());
        out.extend(body.as_bytes());
        out.push(b'\n');
    }
    out
}

/// The other direction of [`encode_source`]. Lengths, not delimiters, decide
/// where each part ends: a markdown file may hold anything at all.
fn decode_source(data: &[u8], what: &str) -> Result<Source, String> {
    let mut at = 0;
    let magic = take_line(data, &mut at, what)?;
    if magic != SOURCE_FORMAT {
        return Err(format!("{}: `{}` is not a source I know", what, magic));
    }
    let mut parts = Vec::new();
    while at < data.len() {
        let header = take_line(data, &mut at, what)?;
        let mut fields = header.split(' ');
        let kind = fields.next().unwrap_or("");
        let name_len: usize = fields
            .next()
            .and_then(|f| f.parse().ok())
            .ok_or_else(|| format!("{}: `{}` has no name length", what, header))?;
        let body_len: usize = fields
            .next()
            .and_then(|f| f.parse().ok())
            .ok_or_else(|| format!("{}: `{}` has no body length", what, header))?;
        if at + name_len + body_len > data.len() {
            return Err(format!("{}: `{}` is cut short", what, kind));
        }
        let name = std::str::from_utf8(&data[at .. at + name_len])
            .map_err(|e| format!("{}: {} name: {}", what, kind, e))?
            .to_string();
        at += name_len;
        let body = std::str::from_utf8(&data[at .. at + body_len])
            .map_err(|e| format!("{}: {}: {}", what, kind, e))?
            .to_string();
        at += body_len;
        if data.get(at) == Some(&b'\n') {
            at += 1;
        }
        parts.push((kind.to_string(), name, body));
    }
    let mut source = Source {
        markdown_name: "document.md".into(),
        markdown: String::new(),
        style_name: "style.toml".into(),
        style: String::new(),
    };
    let mut seen_markdown = false;
    let mut seen_style = false;
    for (kind, name, body) in parts {
        match kind.as_str() {
            "markdown" => {
                source.markdown_name = name;
                source.markdown = body;
                seen_markdown = true;
            }
            "style" => {
                source.style_name = name;
                source.style = body;
                seen_style = true;
            }
            other => return Err(format!("{}: unknown source part `{}`", what, other)),
        }
    }
    if !seen_markdown || !seen_style {
        return Err(format!("{}: the source is missing its markdown or style", what));
    }
    Ok(source)
}

fn take_line<'a>(data: &'a [u8], at: &mut usize, what: &str) -> Result<&'a str, String> {
    let end = data
        .get(*at ..)
        .and_then(|rest| rest.iter().position(|&b| b == b'\n'))
        .ok_or_else(|| format!("{}: the source is cut short", what))?;
    let line = std::str::from_utf8(&data[*at .. *at + end])
        .map_err(|e| format!("{}: {}", what, e))?;
    *at += end + 1;
    Ok(line)
}

/// How many pages the layout produced, which is the number the writer emits.
fn columns_len(state: &State) -> usize {
    let mut cache = Cache::new();
    let _ = cache.layout(&state.storage, &state.design, &state.target, state.root);
    cache.columns.as_ref().map(|c| c.len()).unwrap_or(0)
}

// ------------------------------------------------------------------ fonts

/// What the document used one font for: its glyphs and, for each glyph, the
/// characters it stands for (what `/ToUnicode` has to say).
struct Usage {
    gids: BTreeSet<u16>,
    text: HashMap<u16, String>,
}

impl Usage {
    fn new() -> Usage {
        Usage {
            gids: BTreeSet::new(),
            text: HashMap::new(),
        }
    }
}

struct Plan {
    name: Name,
    subset: cff::Subset,
    /// Glyph id in the original font -> the characters it stands for, for
    /// `/ToUnicode`. This is the document's business, not the font's: the same
    /// subset can serve documents that read differently.
    text: HashMap<u16, String>,
}

fn slot(
    order: &mut Vec<FontId>,
    index: &mut HashMap<FontId, usize>,
    usages: &mut Vec<Usage>,
    id: FontId,
) -> usize {
    if let Some(&i) = index.get(&id) {
        return i;
    }
    let i = order.len();
    order.push(id);
    index.insert(id, i);
    usages.push(Usage::new());
    i
}

/// Record a laid-out run: its glyphs, and the text behind each glyph when the
/// caller knows it. Hyphenated fragments do not (their text is a slice the
/// layout made up), and every glyph in them also appears whole elsewhere.
fn note(usage: &mut Usage, face: &FontFace, layout: &Layout, text: Option<&str>) {
    for (gid, _) in layout.glyphs() {
        usage.gids.insert(gid.0 as u16);
    }
    let text = match text {
        Some(text) => text,
        None => return,
    };
    let indices = grapheme_indices(face, text);
    for (i, (gid, _)) in layout.glyphs().iter().enumerate() {
        let start = indices.get(i).copied().unwrap_or(0);
        let end = indices.get(i + 1).copied().unwrap_or(text.len());
        if start < end {
            usage
                .text
                .entry(gid.0 as u16)
                .or_insert_with(|| text[start..end].to_string());
        }
    }
}

fn plan_fonts(
    state: &State,
    cache: &Cache,
    columns: &Columns,
    subset_fonts: bool,
) -> Result<(Vec<Plan>, HashMap<FontId, usize>), String> {
    let mut order: Vec<FontId> = Vec::new();
    let mut index: HashMap<FontId, usize> = HashMap::new();
    let mut usages: Vec<Usage> = Vec::new();

    for column in columns.columns() {
        for (_y, line) in column {
            for (_x, _size, item, _tag) in line {
                match item {
                    RenderItem::Word(key, part, font) => {
                        if let Some(layout) = cache.word_layout_cache.get(&(font, key, part)) {
                            let word = if part == WordPart::Full {
                                Some(state.storage.get_word(key).text.as_str())
                            } else {
                                None
                            };
                            let i = slot(&mut order, &mut index, &mut usages, font.font_face);
                            note(
                                &mut usages[i],
                                state.storage.get_font_face(font.font_face),
                                layout,
                                word,
                            );
                        }
                    }
                    RenderItem::Margin(key, font, _hang) => {
                        if let Some(layout) =
                            cache.word_layout_cache.get(&(font, key, WordPart::Full))
                        {
                            let i = slot(&mut order, &mut index, &mut usages, font.font_face);
                            note(
                                &mut usages[i],
                                state.storage.get_font_face(font.font_face),
                                layout,
                                Some(state.storage.get_word(key).text.as_str()),
                            );
                        }
                    }
                    RenderItem::Symbol(key, font) => {
                        if let Some(layout) = cache.symbol_layout_cache.get(&(font, key)) {
                            let i = slot(&mut order, &mut index, &mut usages, font.font_face);
                            note(
                                &mut usages[i],
                                state.storage.get_font_face(font.font_face),
                                layout,
                                Some(state.storage.get_symbol(key).text.as_str()),
                            );
                        }
                    }
                    RenderItem::Object(_) | RenderItem::Empty => {}
                }
            }
        }
    }

    let mut plans = Vec::with_capacity(usages.len());
    for (i, (id, usage)) in order.iter().zip(usages.iter()).enumerate() {
        let face = state.storage.get_font_face(*id);
        let data = face.data();
        let mut used = usage.gids.clone();
        if !subset_fonts {
            let sfnt = sfnt::Sfnt::parse(data)?;
            for gid in 0..sfnt.num_glyphs()? {
                used.insert(gid);
            }
        }
        let subset = cff::subset(data, &used)
            .map_err(|e| format!("font {}: {}", subset_name(data), e))?;
        if let Ok(dir) = std::env::var("GRAF_PDF_DUMP_FONTS") {
            let _ = std::fs::write(
                format!("{}/{}.cff", dir, subset.name),
                &subset.cff,
            );
        }
        eprintln!(
            "font {}: {} of {} glyphs, program {} -> {} bytes",
            subset.name,
            subset.used,
            subset.num_glyphs,
            data.len(),
            subset.cff.len()
        );
        plans.push(Plan {
            name: Name::from(format!("F{}", i + 1).as_str()),
            subset,
            text: usage.text.clone(),
        });
    }
    Ok((plans, index))
}

fn subset_name(data: &[u8]) -> String {
    sfnt::Sfnt::parse(data)
        .ok()
        .and_then(|s| s.postscript_name())
        .unwrap_or_else(|| "?".to_string())
}

fn build_font(updater: &mut impl Updater, plan: &Plan) -> Result<PdfFont, String> {
    let subset = &plan.subset;
    // A subset keeps the real name behind the six-letter tag PDF wants.
    let base = format!("GRFLYA+{}", subset.name);
    let base_name = Name::from(base.as_str());

    let program = Stream::new(
        FontStream3 {
            subtype: FontTypeExt::CIDFontType0C,
        },
        &subset.cff,
    )
    .map_err(|e| e.to_string())?;
    let program = updater.create(program).map_err(|e| e.to_string())?;

    let cmap = to_unicode_cmap(plan);
    let to_unicode = Stream::new((), cmap.as_bytes()).map_err(|e| e.to_string())?;
    let to_unicode = updater.create(to_unicode).map_err(|e| e.to_string())?;

    let descriptor = FontDescriptor {
        font_name: base_name.clone(),
        font_family: None,
        font_stretch: None,
        font_weight: None,
        // Symbolic: the font is read through an identity encoding, not an
        // 8-bit text encoding.
        flags: 4,
        font_bbox: Some(Rectangle {
            left: subset.bbox[0],
            bottom: subset.bbox[1],
            right: subset.bbox[2],
            top: subset.bbox[3],
        }),
        italic_angle: subset.italic_angle,
        ascent: Some(subset.ascent),
        descent: Some(subset.descent),
        leading: 0.0,
        cap_height: Some(subset.cap_height),
        xheight: 0.0,
        stem_v: 80.0,
        stem_h: 0.0,
        avg_width: 0.0,
        max_width: 0.0,
        missing_width: 0.0,
        font_file: None,
        font_file2: None,
        font_file3: Some(program),
        char_set: None,
    };

    let cid = PdfFont {
        subtype: FontType::CIDFontType0,
        name: Some(base_name.clone()),
        data: FontData::CIDFontType0(CIDFont {
            system_info: system_info(),
            font_descriptor: descriptor,
            default_width: 1000.0,
            widths: width_array(&subset.widths),
            cid_to_gid_map: Some(CidToGidMap::Identity),
            _other: Dictionary::new(),
        }),
        encoding: None,
        to_unicode: None,
        _other: font_type(),
    };

    let mut other = font_type();
    other.insert("BaseFont", Primitive::Name(base.clone().into()));
    let type0 = PdfFont {
        subtype: FontType::Type0,
        name: Some(base_name),
        data: FontData::Type0(Type0Font {
            descendant_fonts: vec![MaybeRef::Indirect(updater.create(cid).map_err(|e| e.to_string())?)],
            to_unicode: Some(to_unicode),
        }),
        encoding: Some(Encoding {
            base: BaseEncoding::IdentityH,
            differences: HashMap::new(),
        }),
        to_unicode: None,
        _other: other,
    };
    Ok(type0)
}

fn font_type() -> Dictionary {
    let mut other = Dictionary::new();
    other.insert("Type", Primitive::Name("Font".into()));
    other
}

fn system_info() -> Dictionary {
    let mut info = Dictionary::new();
    info.insert("Registry", Primitive::String(PdfString::from("Adobe".to_string())));
    info.insert("Ordering", Primitive::String(PdfString::from("Identity".to_string())));
    info.insert("Supplement", Primitive::Integer(0));
    info
}

/// `/W` in the compact form the spec allows: one CID and a one-element array
/// of its width. Widths are exact, not rounded, so a line of text neither
/// drifts nor needs a spacing correction per glyph.
fn width_array(widths: &[f32]) -> Vec<Primitive> {
    let mut out = Vec::with_capacity(widths.len() * 2);
    for (cid, width) in widths.iter().enumerate() {
        out.push(Primitive::Integer(cid as i64));
        out.push(Primitive::Array(vec![Primitive::Number(*width)]));
    }
    out
}

fn to_unicode_cmap(plan: &Plan) -> String {
    let subset = &plan.subset;
    let mut entries: Vec<(u16, String)> = subset
        .gid_map
        .iter()
        .filter_map(|(&gid, &cid)| plan.text.get(&gid).map(|text| (cid, text.clone())))
        .collect();
    entries.sort();
    entries.dedup_by_key(|(cid, _)| *cid);

    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n\
         <0000> <FFFF>\n\
         endcodespacerange\n",
    );
    for chunk in entries.chunks(100) {
        out.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (cid, text) in chunk {
            let utf16: String = text
                .encode_utf16()
                .map(|unit| format!("{:04X}", unit))
                .collect();
            out.push_str(&format!("<{:04X}> <{}>\n", cid, utf16));
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out
}


// ------------------------------------------------------------------ drawing

struct Content {
    ops: Vec<Op>,
    page_height: f32,
    fill: Option<(u8, u8, u8)>,
}

impl Content {
    fn new(page_height: f32) -> Content {
        Content {
            ops: Vec::new(),
            page_height,
            fill: None,
        }
    }

    fn set_fill(&mut self, color: (u8, u8, u8)) {
        if self.fill == Some(color) {
            return;
        }
        self.fill = Some(color);
        let to = |c: u8| c as f32 / 255.0;
        self.ops.push(Op::FillColor {
            color: Color::Rgb(Rgb {
                red: to(color.0),
                green: to(color.1),
                blue: to(color.2),
            }),
        });
    }

    /// One laid-out run, as text. The subset's CID for a glyph is its index in
    /// the embedded program, so the run is just those two-byte codes; the only
    /// spacing numbers are where the layout put a glyph somewhere the font's
    /// own advance would not (kerning, and nothing else in these fonts).
    fn text(&mut self, plan: &Plan, layout: &Layout, size_mm: f32, at: Vector2F, grey: bool) {
        let glyphs = layout.glyphs();
        if glyphs.is_empty() || size_mm <= 0.0 {
            return;
        }
        let subset = &plan.subset;
        let size_pt = size_mm * PT_PER_MM;
        let base_x = glyphs[0].1.vector.x();
        let mut array: Vec<TextDrawAdjusted> = Vec::with_capacity(glyphs.len() * 2);
        let mut pen = 0.0f32;
        for (gid, tr) in glyphs {
            let cid = match subset.gid_map.get(&(gid.0 as u16)) {
                Some(&cid) => cid,
                None => continue,
            };
            let want = (tr.vector.x() - base_x) / size_mm * 1000.0;
            let delta = want - pen;
            if delta.abs() > 0.01 {
                // A TJ number moves the pen by -number/1000 of the em.
                array.push(TextDrawAdjusted::Spacing(-delta));
                pen = want;
            }
            array.push(TextDrawAdjusted::Text(PdfString::from(
                cid.to_be_bytes().to_vec(),
            )));
            pen += *subset.widths.get(cid as usize).unwrap_or(&0.0);
        }
        if array.is_empty() {
            return;
        }
        self.set_fill(if grey { MARGIN_GREY } else { (0, 0, 0) });
        let x = (at.x() + base_x) * PT_PER_MM;
        let y = self.page_height - at.y() * PT_PER_MM;
        self.ops.push(Op::BeginText);
        self.ops.push(Op::TextFont {
            name: plan.name.clone(),
            size: size_pt,
        });
        self.ops.push(Op::SetTextMatrix {
            matrix: Matrix {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 1.0,
                e: x,
                f: y,
            },
        });
        self.ops.push(Op::TextDrawAdjusted { array });
        self.ops.push(Op::EndText);
    }

    /// An object draws itself into a scene; its paths become filled outlines
    /// at the object's place on the page.
    fn object(&mut self, state: &State, key: ObjectId, tag: Tag, size: Vector2F, at: Vector2F) {
        let object = state.storage.get_object(key);
        let typ = state.storage.get_weave(tag.seq()).typ();
        let mut scene = Scene::new();
        object.draw(
            ObjectCtx {
                storage: &state.storage,
                target: &state.target,
                design: &state.design,
                typ,
            },
            Vector2F::zero(),
            size,
            &mut scene,
        );
        let page_height = self.page_height;
        for n in 0..scene.draw_path_count() {
            let draw_path = scene.get_draw_path(DrawPathId(n)).clone();
            let color = scene.get_paint(draw_path.paint).base_color();
            self.set_fill((color.r, color.g, color.b));
            emit_outline(&mut self.ops, &draw_path.outline, at, page_height);
        }
    }
}

fn map_point(q: Vector2F, at: Vector2F, page_height: f32) -> Point {
    Point {
        x: (q.x() + at.x()) * PT_PER_MM,
        y: page_height - (q.y() + at.y()) * PT_PER_MM,
    }
}

fn emit_outline(ops: &mut Vec<Op>, outline: &Outline, at: Vector2F, page_height: f32) {
    for contour in outline.contours() {
        let map = |p: Vector2F| map_point(p, at, page_height);
        for (i, segment) in contour.iter(ContourIterFlags::empty()).enumerate() {
            if i == 0 {
                ops.push(Op::MoveTo {
                    p: map(segment.baseline.from()),
                });
            }
            match segment.kind {
                SegmentKind::None => {}
                SegmentKind::Line => ops.push(Op::LineTo {
                    p: map(segment.baseline.to()),
                }),
                SegmentKind::Quadratic => {
                    // Same conversion the old exporter did: the control point
                    // pulled a third of the way in on both sides.
                    let current = segment.baseline.from();
                    let control = segment.ctrl.from();
                    let to = segment.baseline.to();
                    let c1 = Vector2F::splat(2. / 3.) * control + Vector2F::splat(1. / 3.) * current;
                    let c2 = Vector2F::splat(2. / 3.) * control + Vector2F::splat(1. / 3.) * to;
                    ops.push(Op::CurveTo {
                        c1: map(c1),
                        c2: map(c2),
                        p: map(to),
                    });
                }
                SegmentKind::Cubic => ops.push(Op::CurveTo {
                    c1: map(segment.ctrl.from()),
                    c2: map(segment.ctrl.to()),
                    p: map(segment.baseline.to()),
                }),
            }
        }
        if contour.is_closed() {
            ops.push(Op::Close);
        }
    }
    ops.push(Op::Fill {
        winding: Winding::NonZero,
    });
}
