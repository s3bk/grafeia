use font::{Font, GlyphId};
use crate::content::FontFace;

/// The OpenType features a run is shaped with: the standard and the contextual
/// ligatures, which is what `GSUB` is being asked for here.
const LIGATURES: &[&[u8; 4]] = &[b"liga", b"clig", b"rlig"];

/// The run's glyphs before any substitution: one per `char`, `.notdef` where the
/// font has no mapping.
fn codepoint_gids(face: &FontFace, text: &str) -> Vec<GlyphId> {
    text.chars()
        .map(|c| face.gid_for_unicode_codepoint(c as u32).unwrap_or(face.get_notdef_gid()))
        .collect()
}

pub fn build_gids(face: &FontFace, text: &str) -> Vec<GlyphId> {
    let gids = codepoint_gids(face, text);
    face.substitute(&gids, LIGATURES)
}

/// The byte index in `text` where each shaped glyph's cluster starts.
///
/// `font` applies substitutions to a run and reports the glyphs, not where each
/// one came from, so a boundary is found by asking whether the shaping is the
/// same when the run is cut there: if shaping both halves and concatenating gives
/// the run that shaping the whole gave, the cut is not inside a substitution
/// window. Words are short, so the n cuts cost n² glyph visits at most, and only
/// when a run actually contains a substitution.
pub fn grapheme_indices(face: &FontFace, text: &str) -> Vec<usize> {
    let gids = codepoint_gids(face, text);
    let shaped = face.substitute(&gids, LIGATURES);
    if shaped.len() == gids.len() {
        // nothing merged, nothing contextual: one cluster per char
        return text.char_indices().map(|(idx, _)| idx).collect();
    }
    let mut indices = Vec::with_capacity(shaped.len());
    indices.push(0);
    for (k, (idx, _)) in text.char_indices().enumerate().skip(1) {
        let mut split = face.substitute(&gids[.. k], LIGATURES);
        split.extend(face.substitute(&gids[k ..], LIGATURES));
        if split == shaped {
            indices.push(idx);
        }
    }
    indices
}
