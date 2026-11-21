//! The markdown an article is written in.
//!
//! A deliberately small subset — `%key value` front matter, `#`/`##` section
//! headings, `+` list items, `*lead.*` italic lead-ins, `\pagebreak` and
//! `\vfill` directives, blank-line separated paragraphs — which is exactly what
//! an article needs, and more than a general markdown importer could tell us
//! about how each piece is meant to look.
//!
//! The parser knows nothing about design: it says what each block *is*, and the
//! style file says what a thing that is looks like.

/// One structural element of an article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// The publication mark at the head of the piece.
    Masthead(String),
    /// The article title.
    Title(String),
    Subtitle(String),
    /// The label in front of the author's name.
    Byline(String),
    Author(String),
    Email(String),
    /// An ornamental rule.
    Rule,
    /// A forced page break: nothing but the demand that what follows starts on
    /// a fresh page.
    Break,
    /// A vertical fill: the space left in the column, so what follows comes to
    /// rest at the foot of the page.
    VFill,
    /// The small label above a section heading.
    Kicker(String),
    /// `# heading`
    Heading(String),
    /// `## heading`
    Subheading(String),
    Paragraph(String),
    ListItem(String),
}

/// Parse the markdown subset into blocks.
///
/// An unknown `%key` is an error rather than a silently dropped line: front
/// matter that is not understood is a typo, and a typo should be seen.
pub fn parse(input: &str) -> Result<Vec<Block>, String> {
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
        } else if let Some(rest) = line.strip_prefix('%') {
            flush(&mut paragraph, &mut blocks);
            let (key, value) = match rest.find(' ') {
                Some(idx) => (&rest[.. idx], rest[idx + 1 ..].trim()),
                None => (rest, ""),
            };
            let value = value.to_string();
            blocks.push(match key {
                "masthead" => Block::Masthead(value),
                "title" => Block::Title(value),
                "subtitle" => Block::Subtitle(value),
                "byline" => Block::Byline(value),
                "author" => Block::Author(value),
                "email" => Block::Email(value),
                "rule" => Block::Rule,
                "kicker" => Block::Kicker(value),
                other => return Err(format!("unknown front matter key `%{}`", other)),
            });
        } else if let Some(rest) = line.strip_prefix("## ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Subheading(rest.trim().into()));
        } else if let Some(rest) = line.strip_prefix("# ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(rest.trim().into()));
        } else if line == "\\pagebreak" || line == "/pagebreak" {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Break);
        } else if line == "\\vfill" || line == "/vfill" {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::VFill);
        } else if let Some(rest) = line.strip_prefix("+ ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::ListItem(rest.trim().into()));
        } else {
            paragraph.push(line);
        }
    }
    flush(&mut paragraph, &mut blocks);
    Ok(blocks)
}

/// The `*lead.* text` form: an italic opening sentence, then running text.
pub fn split_lead(text: &str) -> (Option<&str>, &str) {
    if text.starts_with('*') {
        if let Some(end) = text[1 ..].find('*') {
            let lead = &text[1 .. end + 1];
            let rest = text[end + 2 ..].trim_start();
            return (Some(lead), rest);
        }
    }
    (None, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_and_paragraphs() {
        let blocks =
            parse("%title The Thing\n%rule\n\nfirst line\nsecond line\n\n# Section\n").unwrap();
        assert_eq!(
            blocks,
            vec![
                Block::Title("The Thing".into()),
                Block::Rule,
                Block::Paragraph("first line second line".into()),
                Block::Heading("Section".into()),
            ]
        );
    }

    #[test]
    fn unknown_front_matter_is_an_error() {
        assert!(parse("%nonsense x\n").is_err());
    }

    #[test]
    fn lead_in() {
        let (lead, rest) = split_lead("*Lead.* Then the text.");
        assert_eq!(lead, Some("Lead."));
        assert_eq!(rest, "Then the text.");
        assert_eq!(split_lead("No lead."), (None, "No lead."));
    }
}
