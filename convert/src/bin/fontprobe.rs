//! `fontprobe <font> [codepoint…]` — prints the glyph id the font crate maps a
//! codepoint to, so a mapping can be compared with fontTools or another shaper.
use font::Font;
use grafeia_core::content::FontFace;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("no font given");
    let cps: Vec<u32> = args.map(|a| u32::from_str_radix(a.trim_start_matches("U+").trim_start_matches("0x"), 16).expect("bad codepoint")).collect();
    let face = FontFace::from_path(&path).expect("can't load font");
    for cp in cps {
        let gid = face.gid_for_unicode_codepoint(cp);
        println!("U+{:04X} -> {:?}", cp, gid.map(|g| g.0));
    }
}
