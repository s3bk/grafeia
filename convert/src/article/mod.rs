//! A markdown article and a style file become a laid-out document.
//!
//! This is the half of the converter that knows about articles: what an article
//! is made of ([`markdown`]), what each of those things looks like
//! ([`style`]), and how the two are put together into a [`State`] that the PDF
//! exporter can write.
//!
//! ```no_run
//! use grafeia_convert::article::{build, Style};
//!
//! # fn main() -> Result<(), String> {
//! let style = Style::load("data/glowpunk.toml")?;
//! let markdown = std::fs::read_to_string("data/glowpunk.md")
//!     .map_err(|e| e.to_string())?;
//! let state = build(&markdown, &style)?;
//! # Ok(())
//! # }
//! ```

pub mod markdown;
pub mod style;

pub use markdown::Block;
pub use style::{DisplayKind, Page, Style, Text, TypeSpec};

use std::borrow::Cow;
use std::collections::HashMap;

use grafeia_core::object::svg::SvgObject;
use grafeia_core::*;

use crate::build;

/// A heading must never be left alone at the foot of a page, nor with a single
/// line of a heading that has more than one.
pub const KEEP_WITH_NEXT: f32 = -50000.0;

/// The types the builder asks for by name. A style need not define all of them:
/// one it leaves out looks like the running text.
const USED: &[&str] = &[
    "document",
    "body",
    "masthead",
    "title",
    "subtitle",
    "byline",
    "author",
    "email",
    "rule",
    "kicker",
    "heading",
    "subheading",
    "paragraph",
    "list-item",
    "lead",
    "pagebreak",
    "vfill",
    "paragraph-number",
];

/// What every type inherits from: the running text.
struct Base {
    font: FontId,
    size: f32,
    line_height: f32,
    word_space: (f32, f32, f32),
    indent: f32,
    dictionary: DictId,
    hyphen: Option<SymbolId>,
}

/// Lay a markdown article out with a style.
pub fn build(markdown_text: &str, style: &Style) -> Result<State<'static>, String> {
    let blocks = markdown::parse(markdown_text)?;

    let mut document = Document::new(Storage::new());
    build::symbols(&mut document);
    if style.text.symbols {
        register_symbols(&mut document);
    }

    // The fonts, by the name the style gives them.
    let mut fonts: HashMap<String, FontId> = HashMap::new();
    for name in style.font_paths.keys() {
        let data = style
            .fonts
            .get(name)
            .ok_or_else(|| format!("style: font `{}` was not loaded", name))?;
        fonts.insert(name.clone(), document.add_font(data.clone()));
    }

    let dictionary = match style.text.dictionary.as_str() {
        "en-gb" | "en_gb" | "en-GB" | "british" => document.load_dict(build::DICT_EN_GB),
        other => {
            return Err(format!(
                "style: no dictionary `{}` (this build has en-gb)",
                other
            ))
        }
    };
    let hyphen = document.add_symbol(Symbol {
        text: "\u{2010}".into(),
        leading: false,
        trailing: true,
        overflow_left: 0.0,
        overflow_right: 1.0,
    });

    let empty = TypeSpec::default();
    let body_spec = style.types.get("body").unwrap_or(&empty);
    let size = body_spec
        .size
        .ok_or_else(|| "style: [types.body].size is required".to_string())?;
    let base = Base {
        font: match &body_spec.font {
            Some(name) => *fonts.get(name).ok_or_else(|| {
                format!("style: [types.body].font: no font named `{}`", name)
            })?,
            None => *fonts
                .values()
                .next()
                .ok_or_else(|| "style: [types.body].font is required".to_string())?,
        },
        size,
        line_height: body_spec.line_height.unwrap_or(size * 1.5),
        word_space: body_spec.word_space.unwrap_or((0.6, 1.0, 1.8)),
        indent: body_spec.indent.unwrap_or(0.0),
        dictionary,
        hyphen: Some(hyphen),
    };

    // One type per name the style gives and per name a block can ask for, so a
    // `with-kicker` may point at a name no block uses.
    let mut names: Vec<String> = style.types.keys().cloned().collect();
    for name in USED {
        if !names.iter().any(|n| n == name) {
            names.push((*name).to_string());
        }
    }
    let mut ids: HashMap<String, TypeId> = HashMap::new();
    for name in &names {
        ids.insert(name.clone(), document.create_type(name, Type::new(name)));
    }

    let body = resolve(&base, body_spec, DisplayKind::Paragraph, &fonts)?;
    let mut design = Design::new(style.name.clone(), body.clone());
    for (name, id) in &ids {
        let spec = style.types.get(name).cloned().unwrap_or_default();
        let kind = spec.display.unwrap_or(DisplayKind::Paragraph);
        let td = resolve(&base, &spec, kind, &fonts)?;
        design.set_type(*id, td);
    }
    // The root's own type: it holds the blocks and is not text of its own.
    let document_typ = ids["document"];

    let mut items: Vec<Item> = vec![];
    let mut list_nr = 1;
    let mut para_nr = 0;
    let mut prev_was_kicker = false;

    for block in blocks {
        let mut seq_items: Vec<Item> = vec![];
        let mut typ_name: &str = match &block {
            Block::Masthead(_) => "masthead",
            Block::Title(_) => "title",
            Block::Subtitle(_) => "subtitle",
            Block::Byline(_) => "byline",
            Block::Author(_) => "author",
            Block::Email(_) => "email",
            Block::Rule => "rule",
            Block::Break => "pagebreak",
            Block::VFill => "vfill",
            Block::Kicker(_) => "kicker",
            Block::Heading(_) => "heading",
            Block::Subheading(_) => "subheading",
            Block::Paragraph(_) => "paragraph",
            Block::ListItem(_) => "list-item",
        };
        if prev_was_kicker {
            if let Some(other) = style.types.get(typ_name).and_then(|s| s.with_kicker.as_deref())
            {
                typ_name = other;
            }
        }

        match &block {
            Block::Rule => {
                let spec = style.types.get(typ_name);
                let svg = spec
                    .and_then(|s| s.svg.clone())
                    .ok_or_else(|| "style: [types.rule].svg is required for a `%rule`".to_string())?;
                let width = spec.and_then(|s| s.svg_width).unwrap_or(50.0);
                let key = document.create_object(Object::Svg(SvgObject::new(
                    Scale::Width(Length::mm(width)),
                    svg.into_bytes(),
                )));
                items.push(Item::Object(key));
                prev_was_kicker = false;
                continue;
            }
            Block::Break => {
                prev_was_kicker = false;
            }
            Block::VFill => {
                prev_was_kicker = false;
            }
            Block::Kicker(text) => {
                seq_items.extend(document.create_text(&text.to_uppercase()));
                prev_was_kicker = true;
            }
            Block::Heading(text) | Block::Subheading(text) => {
                seq_items.extend(document.create_text(text));
                prev_was_kicker = false;
            }
            Block::Masthead(text)
            | Block::Title(text)
            | Block::Subtitle(text)
            | Block::Byline(text)
            | Block::Author(text)
            | Block::Email(text) => {
                seq_items.extend(document.create_text(text));
                prev_was_kicker = false;
            }
            Block::Paragraph(text) => {
                prev_was_kicker = false;
                para_nr += 1;
                if style.text.numbers {
                    let number = document.create_word(&para_nr.to_string());
                    let number_seq = document.creat_seq_with_items(
                        ids["paragraph-number"],
                        vec![Item::Margin(number)],
                    );
                    seq_items.push(Item::Sequence(number_seq));
                }
                match markdown::split_lead(text) {
                    (Some(lead), rest) => {
                        let lead_items: Vec<Item> = document.create_text(lead).collect();
                        let lead_id = document.creat_seq_with_items(ids["lead"], lead_items);
                        seq_items.push(Item::Sequence(lead_id));
                        seq_items.extend(document.create_text(rest));
                    }
                    (None, rest) => {
                        seq_items.extend(document.create_text(rest));
                    }
                }
            }
            Block::ListItem(text) => {
                prev_was_kicker = false;
                seq_items.extend(document.create_text(&format!("{}.", list_nr)));
                seq_items.extend(document.create_text(text));
                list_nr += 1;
            }
        }

        let typ = *ids.get(typ_name).unwrap_or(&ids["body"]);
        let id = document.creat_seq_with_items(typ, seq_items);
        items.push(Item::Sequence(id));
    }

    let root = document.creat_seq_with_items(document_typ, items);
    document.set_root(root);

    let target = Target {
        description: style.page.description.clone(),
        content_box: rect(style.page.content),
        media_box: rect(style.page.media),
        trim_box: rect(style.page.media),
        page_color: Color,
    };

    Ok(State {
        storage: Cow::Owned(document.into_storage()),
        design: Cow::Owned(design),
        target: Cow::Owned(target),
        root,
    })
}

fn rect(a: [f32; 4]) -> Rect {
    Rect {
        left: Length::mm(a[0]),
        width: Length::mm(a[2]),
        top: Length::mm(a[1]),
        height: Length::mm(a[3]),
    }
}

/// One type, with everything it does not say itself taken from the running
/// text.
fn resolve(
    base: &Base,
    spec: &TypeSpec,
    kind: DisplayKind,
    fonts: &HashMap<String, FontId>,
) -> Result<TypeDesign, String> {
    let font = match &spec.font {
        Some(name) => *fonts
            .get(name)
            .ok_or_else(|| format!("style: no font named `{}`", name))?,
        None => base.font,
    };
    let size = spec.size.unwrap_or(base.size);
    let indent = spec.indent.unwrap_or(base.indent);
    let (shrink, length, stretch) = spec.word_space.unwrap_or(base.word_space);
    let padding = VerticalPadding {
        above: Length::mm(spec.above.unwrap_or(0.0)),
        below: Length::mm(spec.below.unwrap_or(0.0)),
    };
    let display = match kind {
        DisplayKind::Block => Display::Block(padding),
        DisplayKind::Inline => Display::Inline,
        DisplayKind::Paragraph => Display::Paragraph(Length::mm(indent), padding),
    };
    Ok(TypeDesign {
        display,
        font: Font {
            font_face: font,
            size: Length::mm(size),
        },
        word_space: FlexMeasure {
            shrink: Length::mm(shrink),
            length: Length::mm(length),
            stretch: Length::mm(stretch),
        },
        line_height: Length::mm(spec.line_height.unwrap_or(base.line_height)),
        dictionary: base.dictionary,
        hyphen: if spec.hyphen.unwrap_or(true) {
            base.hyphen
        } else {
            None
        },
        indent: Length::mm(indent),
        column_break_penalty: if spec.keep_with_next.unwrap_or(false) {
            KEEP_WITH_NEXT
        } else {
            0.0
        },
        column_break_before: spec.break_before.unwrap_or(false),
        vertical_fill: spec.vertical_fill.unwrap_or(false),
        margin_hang: Length::mm(spec.margin_hang.unwrap_or(0.0)),
    })
}

/// `create_symbol` treats every character outside the quote and close classes as
/// a standalone token with word space on both sides, which turns "five-person"
/// into "five - person". Register the infix and bracket characters with the
/// glue the text actually wants; `create_text` then reuses them by their text.
fn register_symbols(document: &mut Document) {
    let attach_both = [
        "-", "\u{2013}", "\u{2014}", "@", "/", "\u{00b7}", "\u{00d7}", "&", "+", "=", "#",
        "\u{20ac}", "$", "\u{00a3}", "\u{00b0}", "\\", "|", "~", "_",
    ];
    let attach_right = [")", "]", "}", ">", "%", "\u{2026}"];
    let attach_left = ["(", "[", "{", "<"];
    for text in attach_both.iter() {
        document.add_symbol(Symbol {
            text: (*text).into(),
            leading: true,
            trailing: true,
            overflow_left: 0.0,
            overflow_right: 0.0,
        });
    }
    for text in attach_right.iter() {
        document.add_symbol(Symbol {
            text: (*text).into(),
            leading: false,
            trailing: true,
            overflow_left: 0.0,
            overflow_right: 0.0,
        });
    }
    for text in attach_left.iter() {
        document.add_symbol(Symbol {
            text: (*text).into(),
            leading: true,
            trailing: false,
            overflow_left: 0.0,
            overflow_right: 0.0,
        });
    }
}
