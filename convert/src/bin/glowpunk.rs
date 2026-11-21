//! `glowpunk <input.md> <output.graf>`
//!
//! Builds the article layout for "Our AI Future is Glowpunk": a US Letter
//! page, Inter for the running text, URW Gothic Demi for the display type, an
//! accent rule on the masthead and one design per structural element.
//!
//! The input is a deliberately small markdown subset — `%key value` front
//! matter, `#`/`##` section headings, `+` list items, `*lead.*` italic
//! lead-ins and blank-line separated paragraphs — which is exactly what an
//! article needs and more than the current importer can express.

use grafeia_convert::build;
use grafeia_convert::build::DICT_EN_GB;
use grafeia_core::object::svg::SvgObject;
use grafeia_core::*;
use std::borrow::Cow;
use std::fs::File;
use std::io::BufWriter;

/// A heading must never be left alone at the foot of a page, nor with a
/// single line of a heading that has more than one.
const KEEP_WITH_NEXT: f32 = -50000.0;

/// Fonts travel with the repository: Inter for the running text and URW Gothic
/// Demi for the display type, both freely redistributable.
macro_rules! font {
    ($name:tt) => {
        &include_bytes!(concat!("../../../data/", $name))[..]
    };
}

/// One structural element of the article.
enum Block {
    Masthead(String),
    Title(String),
    Subtitle(String),
    Byline(String),
    Author(String),
    Email(String),
    Rule,
    /// A forced page break: nothing but the demand that what follows starts on
    /// a fresh page.
    Break,
    Kicker(String),
    Heading(String),
    Subheading(String),
    Paragraph(String),
    ListItem(String),
}

fn parse(input: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();

    fn flush(paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>) {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join(" ")));
            paragraph.clear();
        }
    }

    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else if line.starts_with('%') {
            flush(&mut paragraph, &mut blocks);
            let rest = &line[1 ..];
            let (key, value) = match rest.find(' ') {
                Some(idx) => (&rest[.. idx], rest[idx + 1 ..].trim()),
                None => (rest, ""),
            };
            blocks.push(match key {
                "masthead" => Block::Masthead(value.into()),
                "title" => Block::Title(value.into()),
                "subtitle" => Block::Subtitle(value.into()),
                "byline" => Block::Byline(value.into()),
                "author" => Block::Author(value.into()),
                "email" => Block::Email(value.into()),
                "rule" => Block::Rule,
                "kicker" => Block::Kicker(value.into()),
                other => panic!("unknown front matter key %{}", other),
            });
        } else if let Some(rest) = line.strip_prefix("## ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Subheading(rest.trim().into()));
        } else if let Some(rest) = line.strip_prefix("# ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(rest.trim().into()));
        } else if line == "\\pagebreak" {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Break);
        } else if let Some(rest) = line.strip_prefix("+ ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::ListItem(rest.trim().into()));
        } else {
            paragraph.push(line);
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// The `*lead.* text` form: an italic opening sentence, then running text.
fn split_lead(text: &str) -> (Option<&str>, &str) {
    if text.starts_with('*') {
        if let Some(end) = text[1 ..].find('*') {
            let lead = &text[1 .. end + 1];
            let rest = text[end + 2 ..].trim_start();
            return (Some(lead), rest);
        }
    }
    (None, text)
}

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags: Vec<&str> = args.iter().filter(|a| a.starts_with("--")).map(|a| a.as_str()).collect();
    let mut files = args.iter().filter(|a| !a.starts_with("--"));
    let input = std::fs::read_to_string(files.next().expect("no input file given"))
        .expect("can't read input file");
    let output_path = files.next().expect("no output file given").clone();
    // numbers in the left margin, to talk about paragraphs
    let numbers = flags.contains(&"--numbers");
    // print the laid out columns as text, one line per line
    let dump = flags.contains(&"--dump");

    let blocks = parse(&input);

    let mut document = Document::new(Storage::new());
    build::symbols(&mut document);

    // `create_symbol` treats every character outside the quote/close classes as
    // a standalone token with word space on both sides, which turns
    // "five-person" into "five - person". Register the infix and bracket
    // characters with the glue the text actually wants; `create_text` then
    // reuses them by their text.
    let attach_both = ["-", "\u{2013}", "\u{2014}", "@", "/", "\u{00b7}", "\u{00d7}", "&", "+", "=", "#", "\u{20ac}", "$", "\u{00a3}", "\u{00b0}", "\\", "|", "~", "_"];
    let attach_right = [")", "]", "}", ">", "%", "\u{2026}"];
    let attach_left = ["(", "[", "{", "<"];
    for text in attach_both.iter() {
        document.add_symbol(Symbol { text: (*text).into(), leading: true, trailing: true, overflow_left: 0.0, overflow_right: 0.0 });
    }
    for text in attach_right.iter() {
        document.add_symbol(Symbol { text: (*text).into(), leading: false, trailing: true, overflow_left: 0.0, overflow_right: 0.0 });
    }
    for text in attach_left.iter() {
        document.add_symbol(Symbol { text: (*text).into(), leading: true, trailing: false, overflow_left: 0.0, overflow_right: 0.0 });
    }

    let document_typ = document.create_type("document", Type::new("The Article"));
    let masthead_typ = document.create_type("masthead", Type::new("Publication mark"));
    let title_typ = document.create_type("title", Type::new("Article title"));
    let subtitle_typ = document.create_type("subtitle", Type::new("Article subtitle"));
    let byline_typ = document.create_type("byline", Type::new("Byline label"));
    let author_typ = document.create_type("author", Type::new("Author name"));
    let email_typ = document.create_type("email", Type::new("Author email"));
    let kicker_typ = document.create_type("kicker", Type::new("Section kicker"));
    let heading_typ = document.create_type("heading", Type::new("Section heading"));
    let heading_kicker_typ = document.create_type("heading-kicker", Type::new("Section heading under a kicker"));
    let subheading_typ = document.create_type("subheading", Type::new("Subsection heading"));
    let paragraph_typ = document.create_type("paragraph", Type::new("Paragraph"));
    let lead_typ = document.create_type("lead", Type::new("Italic lead-in"));
    let list_item_typ = document.create_type("list-item", Type::new("List item"));
    let pagebreak_typ = document.create_type("pagebreak", Type::new("Forced page break"));
    let para_number_typ = document.create_type("paragraph-number", Type::new("Paragraph number in the margin"));

    let inter = document.add_font(font!("Inter-Regular.otf").to_vec());
    let inter_italic = document.add_font(font!("Inter-Italic.otf").to_vec());
    let gothic = document.add_font(font!("URWGothic-Demi.otf").to_vec());

    let hyphen = document.add_symbol(Symbol {
        text: "‐".into(),
        leading: false,
        trailing: true,
        overflow_left: 0.0,
        overflow_right: 1.0,
    });
    let dictionary = document.load_dict(DICT_EN_GB);

    // 11pt body on a 17pt line, 137mm measure, justified.
    let body = TypeDesign {
        display: Display::Paragraph(
            Length::zero(),
            VerticalPadding {
                above: Length::zero(),
                below: Length::mm(3.6),
            },
        ),
        font: Font {
            font_face: inter,
            size: Length::mm(3.88),
        },
        word_space: FlexMeasure {
            shrink: Length::mm(0.60),
            length: Length::mm(1.00),
            stretch: Length::mm(1.80),
        },
        line_height: Length::mm(6.0),
        indent: Length::zero(),
        dictionary,
        hyphen: Some(hyphen),
        column_break_penalty: 0.0,
        column_break_before: false,
        margin_hang: Length::zero(),
    };

    let mut design = Design::new("Glowpunk article".into(), body.clone());

    let block = |above: f32, below: f32| {
        Display::Block(VerticalPadding {
            above: Length::mm(above),
            below: Length::mm(below),
        })
    };

    design.set_type(
        document_typ,
        TypeDesign {
            display: Display::Inline,
            ..body.clone()
        },
    );
    design.set_type(
        masthead_typ,
        TypeDesign {
            display: block(0.0, 3.2),
            font: Font {
                font_face: gothic,
                size: Length::mm(3.2),
            },
            line_height: Length::mm(4.0),
            word_space: FlexMeasure {
                shrink: Length::mm(0.6),
                length: Length::mm(0.9),
                stretch: Length::mm(2.2),
            },
            ..body.clone()
        },
    );
    design.set_type(
        title_typ,
        TypeDesign {
            display: block(0.0, 3.2),
            font: Font {
                font_face: gothic,
                size: Length::mm(14.0),
            },
            line_height: Length::mm(14.6),
            word_space: FlexMeasure {
                shrink: Length::mm(2.4),
                length: Length::mm(3.2),
                stretch: Length::mm(5.0),
            },
            indent: Length::zero(),
            hyphen: None,
            ..body.clone()
        },
    );
    design.set_type(
        subtitle_typ,
        TypeDesign {
            display: block(0.0, 5.0),
            font: Font {
                font_face: inter,
                size: Length::mm(5.3),
            },
            line_height: Length::mm(7.0),
            ..body.clone()
        },
    );
    design.set_type(
        byline_typ,
        TypeDesign {
            display: block(0.0, 1.2),
            font: Font {
                font_face: gothic,
                size: Length::mm(3.0),
            },
            line_height: Length::mm(3.6),
            ..body.clone()
        },
    );
    design.set_type(
        author_typ,
        TypeDesign {
            display: block(0.0, 0.2),
            font: Font {
                font_face: inter,
                size: Length::mm(4.6),
            },
            line_height: Length::mm(6.0),
            ..body.clone()
        },
    );
    design.set_type(
        email_typ,
        TypeDesign {
            display: block(0.0, 10.0),
            font: Font {
                font_face: inter,
                size: Length::mm(3.88),
            },
            line_height: Length::mm(5.0),
            hyphen: None,
            ..body.clone()
        },
    );
    design.set_type(
        kicker_typ,
        TypeDesign {
            column_break_penalty: KEEP_WITH_NEXT,
            display: block(9.0, 0.8),
            font: Font {
                font_face: gothic,
                size: Length::mm(3.4),
            },
            line_height: Length::mm(4.2),
            hyphen: None,
            ..body.clone()
        },
    );
    design.set_type(
        heading_typ,
        TypeDesign {
            column_break_penalty: KEEP_WITH_NEXT,
            display: block(9.5, 4.2),
            font: Font {
                font_face: gothic,
                size: Length::mm(6.0),
            },
            line_height: Length::mm(7.0),
            word_space: FlexMeasure {
                shrink: Length::mm(1.2),
                length: Length::mm(1.6),
                stretch: Length::mm(3.0),
            },
            hyphen: None,
            ..body.clone()
        },
    );
    design.set_type(
        heading_kicker_typ,
        TypeDesign {
            column_break_penalty: KEEP_WITH_NEXT,
            display: block(1.0, 4.2),
            font: Font {
                font_face: gothic,
                size: Length::mm(6.0),
            },
            line_height: Length::mm(7.0),
            word_space: FlexMeasure {
                shrink: Length::mm(1.2),
                length: Length::mm(1.6),
                stretch: Length::mm(3.0),
            },
            hyphen: None,
            ..body.clone()
        },
    );
    design.set_type(
        subheading_typ,
        TypeDesign {
            column_break_penalty: KEEP_WITH_NEXT,
            display: block(5.5, 2.0),
            font: Font {
                font_face: gothic,
                size: Length::mm(4.1),
            },
            line_height: Length::mm(5.2),
            hyphen: None,
            ..body.clone()
        },
    );
    design.set_type(
        lead_typ,
        TypeDesign {
            display: Display::Inline,
            font: Font {
                font_face: inter_italic,
                size: Length::mm(3.88),
            },
            ..body.clone()
        },
    );
    design.set_type(
        list_item_typ,
        TypeDesign {
            display: Display::Paragraph(
                Length::zero(),
                VerticalPadding {
                    above: Length::zero(),
                    below: Length::mm(2.2),
                },
            ),
            indent: Length::zero(),
            ..body.clone()
        },
    );

    // The forced break writes nothing: the type carries the demand, and the
    // break merges into the break point that is already between the blocks.
    design.set_type(
        pagebreak_typ,
        TypeDesign {
            display: Display::Inline,
            column_break_before: true,
            ..body.clone()
        },
    );
    // Paragraph numbers: small, in the outer margin, hung off the text edge and
    // taking no width, so the line they start is laid out as if they were not
    // there. They start a quarter of the side margin out from the text edge,
    // which leaves them a clear column of their own, lined up on the left.
    design.set_type(
        para_number_typ,
        TypeDesign {
            display: Display::Inline,
            font: Font {
                font_face: gothic,
                size: Length::mm(2.98),
            },
            line_height: Length::mm(6.0),
            hyphen: None,
            margin_hang: Length::mm(39.4 / 4.0),
            ..body.clone()
        },
    );

    let mut items = vec![];
    let mut nr = 1;
    let mut para_nr = 0;
    let mut prev_was_kicker = false;
    for block in blocks {
        let mut seq_items: Vec<Item> = vec![];
        let typ = match block {
            Block::Masthead(t) => { seq_items.extend(document.create_text(&t)); masthead_typ },
            Block::Title(t) => { seq_items.extend(document.create_text(&t)); title_typ },
            Block::Subtitle(t) => { seq_items.extend(document.create_text(&t)); subtitle_typ },
            Block::Byline(t) => { seq_items.extend(document.create_text(&t)); byline_typ },
            Block::Author(t) => { seq_items.extend(document.create_text(&t)); author_typ },
            Block::Email(t) => { seq_items.extend(document.create_text(&t)); email_typ },
            Block::Rule => {
                let rule = concat!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="2.4" viewBox="0 0 200 2.4">"#,
                    r#"<rect x="0" y="0" width="200" height="2.4" fill="#b8541e"/></svg>"##
                );
                let key = document.create_object(Object::Svg(SvgObject::new(
                    Scale::Width(Length::mm(50.8)),
                    rule.as_bytes().to_vec(),
                )));
                items.push(Item::Object(key));
                continue;
            }
            Block::Break => { prev_was_kicker = false; pagebreak_typ },
            Block::Kicker(t) => { seq_items.extend(document.create_text(&t.to_uppercase())); prev_was_kicker = true; kicker_typ },
            Block::Heading(t) => {
                seq_items.extend(document.create_text(&t));
                if prev_was_kicker {
                    prev_was_kicker = false;
                    heading_kicker_typ
                } else {
                    heading_typ
                }
            }
            Block::Subheading(t) => { prev_was_kicker = false; seq_items.extend(document.create_text(&t)); subheading_typ },
            Block::Paragraph(t) => {
                prev_was_kicker = false;
                para_nr += 1;
                if numbers {
                    let number = document.create_word(&para_nr.to_string());
                    let number_seq = document.creat_seq_with_items(
                        para_number_typ,
                        vec![Item::Margin(number)],
                    );
                    seq_items.push(Item::Sequence(number_seq));
                }
                match split_lead(&t) {
                    (Some(lead), rest) => {
                        let lead_items: Vec<Item> = document.create_text(lead).collect();
                        let lead_id = document.creat_seq_with_items(lead_typ, lead_items);
                        seq_items.push(Item::Sequence(lead_id));
                        seq_items.extend(document.create_text(rest));
                    }
                    (None, rest) => {
                        seq_items.extend(document.create_text(rest));
                    }
                }
                paragraph_typ
            }
            Block::ListItem(t) => {
                prev_was_kicker = false;
                seq_items.extend(document.create_text(&format!("{}.", nr)));
                seq_items.extend(document.create_text(&t));
                nr += 1;
                list_item_typ
            }
        };
        let id = document.creat_seq_with_items(typ, seq_items);
        items.push(Item::Sequence(id));
    }

    let root = document.creat_seq_with_items(document_typ, items);
    document.set_root(root);

    let target = Target {
        description: "US Letter, 1.55in side margins".into(),
        content_box: Rect {
            left: Length::mm(39.4),
            width: Length::mm(137.1),
            top: Length::mm(29.2),
            height: Length::mm(222.3),
        },
        media_box: Rect {
            left: Length::mm(0.0),
            width: Length::mm(215.9),
            top: Length::mm(0.0),
            height: Length::mm(279.4),
        },
        trim_box: Rect {
            left: Length::mm(0.0),
            width: Length::mm(215.9),
            top: Length::mm(0.0),
            height: Length::mm(279.4),
        },
        page_color: Color,
    };

    let state = State {
        storage: Cow::Owned(document.into_storage()),
        design: Cow::Owned(design),
        target: Cow::Owned(target),
        root,
    };
    if dump {
        dump_lines(&state, flags.contains(&"--x"));
    }

    let output = File::create(output_path).expect("can't create output file");
    state.store(BufWriter::new(output)).unwrap();
}

/// Print the laid out columns as text, one line per line. This is how a
/// heading left at the foot of a page, a one-line widow or a stray space is
/// found without reading the rendered pages.
fn dump_lines(state: &State<'_>, debug_x: bool) {
    use grafeia_core::draw::{Cache, RenderItem, WordPart};

    let mut cache = Cache::new();
    cache.layout(&state.storage, &state.design, &state.target, state.root);

    for (nr, column) in cache.columns.as_ref().unwrap().columns().enumerate() {
        println!("--- column {}", nr + 1);
        for (_y, line) in column {
            let mut text = String::new();
            let mut end = Length::zero();
            let mut first = true;
            for (x, size, item, _tag) in line {
                if first {
                    first = false;
                    if debug_x {
                        text.push_str(&format!("[{:.3}|{:.3}] ", x.value, size.width.value));
                    }
                }
                if !text.is_empty() && x - end > Length::mm(0.4) {
                    text.push(' ');
                }
                match item {
                    RenderItem::Word(key, WordPart::Before(i), _) => {
                        let word = &state.storage.get_word(key).text;
                        text.push_str(&word[.. i as usize]);
                    }
                    RenderItem::Word(key, WordPart::After(i), _) => {
                        let word = &state.storage.get_word(key).text;
                        text.push_str(&word[i as usize ..]);
                    }
                    RenderItem::Word(key, _, _) => {
                        text.push_str(&state.storage.get_word(key).text)
                    }
                    RenderItem::Symbol(key, _) => text.push_str(&state.storage.get_symbol(key).text),
                    RenderItem::Margin(..) => {}
                    RenderItem::Object(_) => text.push_str("<object>"),
                    RenderItem::Empty => {}
                }
                end = x + size.width;
            }
            println!("{}", text.trim_end());
        }
    }
}
