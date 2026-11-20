use grafeia_convert::build;
use grafeia_core::*;
use grafeia_core::draw::Cache;
use pathfinder_export::{Export, FileFormat};
use std::fs::File;
use std::io::BufReader;

/// `svg [input.graf [prefix]]`
///
/// Writes one SVG per column: `demo-1.svg`, `demo-2.svg`, … Without an input the
/// demo document from `grafeia_convert::build` is laid out directly, so the two
/// demo binaries agree: `demo` stores the document, `svg` draws it.
fn main() {
    env_logger::init();

    let mut args = std::env::args().skip(1);
    let input = args.next();
    let prefix = args.next().unwrap_or_else(|| "demo".to_owned());

    let state: State<'static> = match input {
        Some(path) => State::load(BufReader::new(File::open(&path).expect("can't open input"))).unwrap(),
        None => build::build(),
    };

    let mut cache = Cache::new();
    cache.layout(&state.storage, &state.design, &state.target, state.root);

    let pages = cache.columns.as_ref().unwrap().columns().count();
    for page_nr in 0..pages {
        let page = cache.render_page(&state.storage, &state.target, &state.design, page_nr);
        let path = format!("{}-{}.svg", prefix, page_nr + 1);
        let mut out = File::create(&path).expect("can't create output file");
        page.scene().export(&mut out, FileFormat::SVG).expect("export failed");
        println!("{}: {} bytes", path, std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
    }
}
