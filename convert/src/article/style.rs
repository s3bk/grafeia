//! The style file: what each kind of block looks like.
//!
//! A style is TOML, because a style is a table of small facts and a table is
//! what TOML is for. Every length is in millimetres, and every type inherits
//! from `[types.body]`, so a style says how the running text sits and then only
//! how each other kind of block differs from it.
//!
//! ```toml
//! [page]
//! description = "US Letter"
//! media   = [0.0, 0.0, 215.9, 279.4]     # left, top, width, height
//! content = [39.4, 29.2, 137.1, 222.3]
//!
//! [fonts]
//! text = "Inter-Regular.otf"             # relative to this file
//!
//! [types.body]
//! font = "text"
//! size = 3.88                            # 11pt
//! line-height = 6.0                      # 17pt
//! word-space = [0.60, 1.00, 1.80]        # shrink, length, stretch
//! below = 3.6
//!
//! [types.title]
//! font = "display"
//! size = 14.0
//! line-height = 14.6
//! display = "block"
//! below = 3.2
//! ```
//!
//! Per type, the keys are: `font`, `size`, `line-height`, `word-space`,
//! `indent`, `above`, `below`, `display` (`inline`, `block` or `paragraph`),
//! `hyphen`, `keep-with-next`, `break-before`, `vertical-fill`, `margin-hang`,
//! `svg` and `svg-width` (a block that is a drawing rather than text), and
//! `with-kicker` (the type to use for this one when it directly follows a
//! kicker). An unknown key is an error, so a typo is seen instead of ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How a type takes its place in the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayKind {
    /// A block of its own that may not be broken into paragraphs.
    Block,
    /// Part of the flow around it.
    Inline,
    /// A paragraph: a block that may still be broken across columns.
    Paragraph,
}

/// One kind of block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeSpec {
    pub font: Option<String>,
    /// Type size, mm.
    pub size: Option<f32>,
    /// Baseline to baseline, mm.
    pub line_height: Option<f32>,
    /// Shrink, length and stretch of a word space, mm.
    pub word_space: Option<(f32, f32, f32)>,
    pub indent: Option<f32>,
    pub above: Option<f32>,
    pub below: Option<f32>,
    pub display: Option<DisplayKind>,
    pub hyphen: Option<bool>,
    pub keep_with_next: Option<bool>,
    pub break_before: Option<bool>,
    pub vertical_fill: Option<bool>,
    /// How far into the margin a hung word starts, mm.
    pub margin_hang: Option<f32>,
    /// An SVG drawing, for a block that is a picture rather than text.
    pub svg: Option<String>,
    /// The width the drawing is scaled to, mm.
    pub svg_width: Option<f32>,
    /// The type to use for this one when it follows a kicker.
    pub with_kicker: Option<String>,
}

/// The paper.
#[derive(Debug, Clone)]
pub struct Page {
    pub description: String,
    /// left, top, width, height; mm.
    pub media: [f32; 4],
    /// left, top, width, height; mm.
    pub content: [f32; 4],
}

impl Default for Page {
    fn default() -> Self {
        // US Letter with the side margins the article wants.
        Page {
            description: "US Letter".into(),
            media: [0.0, 0.0, 215.9, 279.4],
            content: [39.4, 29.2, 137.1, 222.3],
        }
    }
}

/// What the text is measured and hyphenated with.
#[derive(Debug, Clone)]
pub struct Text {
    pub dictionary: String,
    /// Register the infix and bracket characters with the glue text wants, so
    /// "five-person" is one word and not "five - person".
    pub symbols: bool,
    /// Number the paragraphs in the margin, to be able to talk about them.
    pub numbers: bool,
}

impl Default for Text {
    fn default() -> Self {
        Text {
            dictionary: "en-gb".into(),
            symbols: true,
            numbers: false,
        }
    }
}

/// A style: the paper, the text, the fonts and one entry per kind of block.
#[derive(Debug, Clone, Default)]
pub struct Style {
    pub name: String,
    pub page: Page,
    pub text: Text,
    pub types: HashMap<String, TypeSpec>,
    /// Where each font lives, as written in the file.
    pub font_paths: HashMap<String, PathBuf>,
    /// The fonts themselves, once loaded.
    pub fonts: HashMap<String, Vec<u8>>,
}

impl Style {
    /// Parse a style from TOML. Fonts are not read yet.
    pub fn parse(text: &str) -> Result<Style, String> {
        let root: toml::Value = text
            .parse()
            .map_err(|e| format!("style: not valid TOML: {}", e))?;
        let root = root
            .as_table()
            .ok_or_else(|| "style: the file must be a table".to_string())?;

        let mut style = Style::default();
        style.name = "Untitled".into();

        for (key, value) in root {
            match key.as_str() {
                "name" => style.name = string(value, "name")?,
                "page" => style.page = page(value)?,
                "text" => style.text = text_spec(value)?,
                "fonts" => {
                    let table = table(value, "fonts")?;
                    for (name, path) in table {
                        style
                            .font_paths
                            .insert(name.clone(), PathBuf::from(string(path, "fonts")?));
                    }
                }
                "types" => {
                    let table = table(value, "types")?;
                    for (name, spec) in table {
                        style
                            .types
                            .insert(name.clone(), type_spec(name, spec)?);
                    }
                }
                other => return Err(format!("style: unknown section `[{}]`", other)),
            }
        }
        Ok(style)
    }

    /// Read a style file, and the fonts it names beside it.
    pub fn load(path: impl AsRef<Path>) -> Result<Style, String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{}: {}", path.display(), e))?;
        let mut style =
            Style::parse(&text).map_err(|e| format!("{}: {}", path.display(), e))?;
        let base = path.parent().unwrap_or(Path::new(""));
        for (name, font) in style.font_paths.clone() {
            let file = if font.is_absolute() { font } else { base.join(font) };
            let data =
                std::fs::read(&file).map_err(|e| format!("{}: {}", file.display(), e))?;
            style.fonts.insert(name, data);
        }
        Ok(style)
    }
}

fn page(value: &toml::Value) -> Result<Page, String> {
    let table = table(value, "page")?;
    let mut page = Page::default();
    for (key, value) in table {
        match key.as_str() {
            "description" => page.description = string(value, "page.description")?,
            "media" => page.media = rect(value, "page.media")?,
            "content" => page.content = rect(value, "page.content")?,
            other => return Err(format!("style: [page]: unknown key `{}`", other)),
        }
    }
    Ok(page)
}

fn text_spec(value: &toml::Value) -> Result<Text, String> {
    let table = table(value, "text")?;
    let mut text = Text::default();
    for (key, value) in table {
        match key.as_str() {
            "dictionary" => text.dictionary = string(value, "text.dictionary")?,
            "symbols" => text.symbols = boolean(value, "text.symbols")?,
            "numbers" => text.numbers = boolean(value, "text.numbers")?,
            other => return Err(format!("style: [text]: unknown key `{}`", other)),
        }
    }
    Ok(text)
}

fn type_spec(name: &str, value: &toml::Value) -> Result<TypeSpec, String> {
    let table = table(value, "types")?;
    let where_ = |key: &str| format!("[types.{}].{}", name, key);
    let mut spec = TypeSpec::default();
    for (key, value) in table {
        match key.as_str() {
            "font" => spec.font = Some(string(value, &where_(key))?),
            "size" => spec.size = Some(number(value, &where_(key))?),
            "line-height" => spec.line_height = Some(number(value, &where_(key))?),
            "word-space" => {
                let a = array(value, &where_(key))?;
                if a.len() != 3 {
                    return Err(format!("{}: three numbers, shrink, length, stretch", where_(key)));
                }
                spec.word_space = Some((
                    number(&a[0], &where_(key))?,
                    number(&a[1], &where_(key))?,
                    number(&a[2], &where_(key))?,
                ));
            }
            "indent" => spec.indent = Some(number(value, &where_(key))?),
            "above" => spec.above = Some(number(value, &where_(key))?),
            "below" => spec.below = Some(number(value, &where_(key))?),
            "display" => {
                let kind = string(value, &where_(key))?;
                spec.display = Some(match kind.as_str() {
                    "block" => DisplayKind::Block,
                    "inline" => DisplayKind::Inline,
                    "paragraph" => DisplayKind::Paragraph,
                    other => {
                        return Err(format!(
                            "{}: `{}` is not a display (block, inline or paragraph)",
                            where_(key),
                            other
                        ))
                    }
                });
            }
            "hyphen" => spec.hyphen = Some(boolean(value, &where_(key))?),
            "keep-with-next" => spec.keep_with_next = Some(boolean(value, &where_(key))?),
            "break-before" => spec.break_before = Some(boolean(value, &where_(key))?),
            "vertical-fill" => spec.vertical_fill = Some(boolean(value, &where_(key))?),
            "margin-hang" => spec.margin_hang = Some(number(value, &where_(key))?),
            "svg" => spec.svg = Some(string(value, &where_(key))?),
            "svg-width" => spec.svg_width = Some(number(value, &where_(key))?),
            "with-kicker" => spec.with_kicker = Some(string(value, &where_(key))?),
            other => return Err(format!("style: [types.{}]: unknown key `{}`", name, other)),
        }
    }
    Ok(spec)
}

fn table<'a>(value: &'a toml::Value, what: &str) -> Result<&'a toml::value::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("style: `{}` must be a table", what))
}

fn array<'a>(value: &'a toml::Value, what: &str) -> Result<&'a Vec<toml::Value>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("style: `{}` must be an array", what))
}

fn string(value: &toml::Value, what: &str) -> Result<String, String> {
    value
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("style: `{}` must be a string", what))
}

fn boolean(value: &toml::Value, what: &str) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("style: `{}` must be true or false", what))
}

fn number(value: &toml::Value, what: &str) -> Result<f32, String> {
    match value {
        toml::Value::Integer(i) => Ok(*i as f32),
        toml::Value::Float(f) => Ok(*f as f32),
        _ => Err(format!("style: `{}` must be a number", what)),
    }
}

fn rect(value: &toml::Value, what: &str) -> Result<[f32; 4], String> {
    let a = array(value, what)?;
    if a.len() != 4 {
        return Err(format!("style: `{}`: four numbers, left, top, width, height", what));
    }
    Ok([
        number(&a[0], what)?,
        number(&a[1], what)?,
        number(&a[2], what)?,
        number(&a[3], what)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_style() {
        let style = Style::parse(
            r#"
            name = "Test"
            [page]
            media = [0.0, 0.0, 210.0, 297.0]
            [fonts]
            text = "a.otf"
            [types.body]
            font = "text"
            size = 4.0
            display = "paragraph"
            "#,
        )
        .unwrap();
        assert_eq!(style.name, "Test");
        assert_eq!(style.page.media, [0.0, 0.0, 210.0, 297.0]);
        assert_eq!(style.font_paths["text"], PathBuf::from("a.otf"));
        assert_eq!(style.types["body"].size, Some(4.0));
        assert_eq!(style.types["body"].display, Some(DisplayKind::Paragraph));
    }

    #[test]
    fn unknown_keys_are_errors() {
        assert!(Style::parse("[types.body]\nnonsense = 1\n").is_err());
        assert!(Style::parse("[nonsense]\n").is_err());
    }
}
