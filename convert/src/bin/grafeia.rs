//! `grafeia` — a markdown article and a style file become a PDF.
//!
//! ```text
//! grafeia article.md style.toml article.pdf
//! grafeia --extract article.pdf .
//! ```
//!
//! The markdown says what the article is made of, the style says what each kind
//! of thing looks like, and the layout engine puts the two together. Nothing in
//! this binary knows about either: it reads the two files, calls
//! [`article::build`] and hands the result to [`export::pdf::write`].
//!
//! The PDF carries the markdown and the style it was made from, so it can be
//! opened again later with `--extract` and edited with the very text and style
//! it was set from.

use std::process::exit;

use grafeia_convert::article;
use grafeia_convert::export::pdf;
use grafeia_core::draw::{Cache, RenderItem, WordPart};
use grafeia_core::*;

const USAGE: &str = "\
usage: grafeia [options] <input.md> <style.toml> <output.pdf>
       grafeia --extract <input.pdf> [<directory>]

options:
  --numbers        number the paragraphs in the left margin, to talk about them
  --no-numbers     turn margin numbering off, whatever the style says
  --no-subset      embed the fonts whole instead of the glyphs used
  --no-source      do not carry the markdown and style inside the PDF
  --graf <path>    also write the laid out document as a .graf file
  --dump           print the laid out columns as text, one line per line
  --x              with --dump, print each line's x and width
  --extract        write the markdown and style out of a PDF that carries them
";

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", USAGE);
        exit(if args.is_empty() { 2 } else { 0 });
    }
    if let Err(message) = run(&args) {
        eprintln!("grafeia: {}", message);
        exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut numbers: Option<bool> = None;
    let mut subset = true;
    let mut source = true;
    let mut dump = false;
    let mut debug_x = false;
    let mut extract = false;
    let mut graf: Option<String> = None;
    let mut files: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--numbers" => numbers = Some(true),
            "--no-numbers" => numbers = Some(false),
            "--no-subset" => subset = false,
            "--no-source" => source = false,
            "--dump" => dump = true,
            "--x" => debug_x = true,
            "--extract" => extract = true,
            "--graf" => {
                i += 1;
                graf = Some(
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| "--graf needs a path".to_string())?,
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option `{}`\n\n{}", other, USAGE))
            }
            other => files.push(other.to_string()),
        }
        i += 1;
    }

    if extract {
        let path = files
            .first()
            .ok_or_else(|| format!("--extract needs a PDF\n\n{}", USAGE))?;
        let dir = std::path::Path::new(files.get(1).map(String::as_str).unwrap_or("."));
        return extract_source(path, dir);
    }

    if files.len() < 3 {
        return Err(format!("need an input, a style and an output\n\n{}", USAGE));
    }
    let input_path = &files[0];
    let style_path = &files[1];
    let output_path = &files[2];

    let markdown = std::fs::read_to_string(input_path)
        .map_err(|e| format!("{}: {}", input_path, e))?;
    let mut style = article::Style::load(style_path)?;
    if let Some(numbers) = numbers {
        style.text.numbers = numbers;
    }

    let state = article::build(&markdown, &style)?;

    if dump {
        dump_lines(&state, debug_x);
    }
    if let Some(path) = &graf {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {}", path, e))?;
        state
            .store(std::io::BufWriter::new(file))
            .map_err(|e| format!("{}: {}", path, e))?;
    }

    let options = pdf::Options {
        subset_fonts: subset,
        source: if source {
            Some(pdf::Source {
                markdown_name: file_name(input_path),
                markdown: markdown.clone(),
                style_name: file_name(style_path),
                style: std::fs::read_to_string(style_path)
                    .map_err(|e| format!("{}: {}", style_path, e))?,
            })
        } else {
            None
        },
    };

    pdf::write_file(&state, output_path, &options)?;
    if !dump {
        eprintln!(
            "grafeia: {} -> {}",
            input_path, output_path
        );
    }
    Ok(())
}

/// `--extract`: the source parts a PDF carries, written out under their names.
fn extract_source(path: &str, dir: &std::path::Path) -> Result<(), String> {
    let source = pdf::extract(path)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let markdown = dir.join(&source.markdown_name);
    let style = dir.join(&source.style_name);
    std::fs::write(&markdown, &source.markdown)
        .map_err(|e| format!("{}: {}", markdown.display(), e))?;
    std::fs::write(&style, &source.style)
        .map_err(|e| format!("{}: {}", style.display(), e))?;
    eprintln!(
        "grafeia: {} + {}",
        markdown.display(),
        style.display()
    );
    Ok(())
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_string())
}

/// Print the laid out columns as text, one line per line. This is how a heading
/// left at the foot of a page, a one-line widow or a stray space is found
/// without reading the rendered pages.
fn dump_lines(state: &State<'_>, debug_x: bool) {
    let mut cache = Cache::new();
    cache.layout(&state.storage, &state.design, &state.target, state.root);

    let columns = match cache.columns.as_ref() {
        Some(columns) => columns,
        None => {
            eprintln!("grafeia: the layout produced no columns");
            return;
        }
    };
    for (nr, column) in columns.columns().enumerate() {
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
                    RenderItem::Symbol(key, _) => {
                        text.push_str(&state.storage.get_symbol(key).text)
                    }
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
