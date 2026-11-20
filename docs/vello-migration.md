# Updating grafeia to vello

Status: plan, 2026-11-20. Based on `master` (5185766, 2020-03-26).

## Summary

The engine does not need a rewrite to sit on vello. Three things touch pathfinder:
the *scene* it draws into (`pathfinder_renderer::scene::Scene`), the *paths* it
draws (`pathfinder_content::outline::Outline`), and the *shell* that presents
scenes (`pathfinder_view`, GL/WebGL). Geometry (`pathfinder_geometry`) is a leaf
type crate and is not part of the problem.

So: keep geometry and glyph outlines as they are, replace the scene with a vello
scene behind the existing `vector::Surface` abstraction, replace `pathfinder_svg`
with a small usvg walker, and write a `grafeia_view` shell on winit/wgpu (native)
and vello_cpu + canvas (wasm). `pdf_render` already does exactly this in
`render/src/vello_backend.rs` and `render/src/vello_cpu_backend.rs`, with
`outline_to_bez()` as the seam — that is the template to copy, including the
version pins (`vello = "=0.11.0"`, `vello_cpu = "0.3"`, `peniko = "0.6"`).

Before any of that, the workspace has to load and the core has to compile on a
current toolchain; that is a separate, small piece of work (stage 0) and is
needed regardless.

## Where it stands

- 5 418 lines of Rust: core 3 454 (layout/typography engine), app 765, convert 977,
  server 151, web 71. Layout is renderer-independent; rendering is 3 functions.
- The document model is pathfinder-free: only the page/svg/tex objects and
  `draw.rs` know about scenes.
- Dependencies are all 2020: `pathfinder` (servo, archived 2021), `vector`
  (s3bk, last push 2020-04, has `impl_pathfinder` / `impl_raqote` / `impl_svg`),
  `pathfinder_view` (s3bk, 2023-07), `usvg 0.9`, `font` before the encoder API.

## What blocks a build today

1. `Cargo.toml` lists members `rex` and `tex`; neither directory exists. `rex`
   is the `grafeia/ReX` repo (last push 2022-02-12, itself on old pathfinder),
   `tex` never existed. `cargo metadata` fails on `rex/Cargo.toml`.
2. `core/src/lib.rs:1` is `#![feature(generators, generator_trait, entry_insert)]`
   — nightly-only, and `generators` no longer exists as a gate (coroutines, and
   they need the `#[coroutine]` attribute). `entry_insert` is stable and unused.
   So core compiles on no toolchain that exists now.
   - `entry_insert`: zero call sites (`or_insert` / `or_insert_with` only) — drop.
   - `generators`: only `core/src/gen.rs` (32 lines, `GenIter`), used at
     `document.rs:167` (`items()`, a fold over CRDT atoms holding one pending
     item) and `document.rs:585` (`childen()`, a DFS with an explicit stack).
     Both are plain iterator work — `from_fn` for the first, a small state
     machine for the second. ~50 lines. `writer.rs`'s `Gen2`/`branch2` is
     *not* a generator, it is closures; it is fine.
   - `convert/src/bin/layout.rs:1` is `#![feature(test)]` (a criterion-less
     bench); move it behind a bench target or delete.
3. `usvg 0.9` (2020) against resvg 0.44 API (`Tree::from_data` still exists,
   `Options` and the node tree do not).
4. `font` has moved to the encoder API (`font::parse(&data, encoder)`,
   `Encoder`/`Pen`, `GlyphRef`), and `content.rs` uses the old
   `font::parse::<Outline>(&data)` / `Box<dyn font::Font<Outline>>`.

## Coupling inventory

| now | replacement | where |
| --- | --- | --- |
| `pathfinder_renderer::scene::Scene` | `vello::Scene` / `vello_cpu::RenderContext` behind `vector::Surface` | draw.rs (Page), object/{mod,svg,tex}.rs, app/app.rs, app/net.rs |
| `pathfinder_content::outline::Outline` | **unchanged**; converted to `kurbo::BezPath` at draw time | content.rs, draw.rs, object/tex.rs, convert/bin/pdf.rs |
| `vector::{Surface,PathStyle,Paint,PathBuilder,FillRule}` | same API, new `impl_vello` / `impl_vello_cpu` | draw.rs, app/app.rs, object/tex.rs |
| `pathfinder_svg::BuiltSVG` | own `usvg::Tree` walker → `(BezPath, Brush)` list | object/svg.rs (100 lines) |
| `pathfinder_view::{Interactive,Context}` | `grafeia_view` with the same 7-method trait | app/app.rs, net.rs, browser.rs, web/lib.rs, bin/{local,remote}.rs |
| `pathfinder_export` | unused — delete from manifests | core, app |
| `pathfinder_geometry` | **unchanged** | everything |
| `usvg 0.9` | `usvg 0.44` (what pdf_render's dev-deps use) | object/svg.rs |

`pathfinder_geometry` staying is a decision, not an oversight: it is ~2 k lines of
`Vector2F`/`RectF`/`Transform2F`, no GPU, no scene, and it is the coordinate
vocabulary of the whole engine and of `font`'s `Pen` trait. Canonicalising on
kurbo later is a separate, optional change (and it would have to start in the
`font` repo, since `Pen` takes `pathfinder_geometry::Vector2F`).

## Decisions

1. **Paths stay pathfinder `Outline`; convert at the scene boundary.**
   `outline_to_bez()` (~40 lines, already written in pdf_render) maps
   `contour.iter(ContourIterFlags::empty())` + `SegmentKind` to
   `move_to/line_to/quad_to/curve_to/close_path`. One copy in `vector`, next to
   the impls; grafeia's `Layout::render`, `PathBuilder` uses and pdf import all
   keep working untouched. Cost is one BezPath per drawn path per frame — for a
   page of text that is a few thousand lines, not a problem; if it ever is,
   cache the BezPath per glyph next to the outline.

2. **Coordinates stay y-down internally; flip once.**
   grafeia lays out in mm with the origin at the media box's top-left and the
   y axis pointing down, because pathfinder is a screen-space renderer;
   `build_word_layout` already flips glyphs with `from_scale(1, -1)` into that
   space. kurbo is y-up. Do not convert the engine: apply
   `Affine::translate((0.0, page_height)) * Affine::scale_non_uniform(1.0, -1.0)`
   in the one place that hands the page to the renderer. Everything above it
   (layout, hit testing, cursor, pdf writer) is unchanged, which is what makes
   this cheap.

3. **Two backends, `vello_cpu` is the default.**
   `vello` (wgpu) needs a GPU and, on wasm, WebGPU — Chrome/Edge only, where the
   demo runs on WebGL2 today. `vello_cpu` is pure Rust, takes the same kurbo
   paths, and runs everywhere including wasm, at a few tens of ms per page —
   right for a paged reader, not for smooth zoom. So: `vello_cpu` for wasm and
   for headless rendering (PNG out, the thing tests compare), `vello`+wgpu as an
   optional native fast path. This is the same split pdf_render ships.

4. **The new drawing glue lives in `vector`, not in grafeia.**
   `vector` exists for exactly this: `Surface`/`PathStyle`/`Paint`/`PathBuilder`
   is a small backend-neutral API with three impls. A fourth and fifth
   (`impl_vello`, `impl_vello_cpu`, ~200 lines total, mirroring
   `impl_pathfinder.rs`) keep `draw.rs` and `app.rs` unchanged. It is a private
   crate with no other consumers, so this costs nobody anything.

5. **The shell is new code, not a fork.** `pathfinder_view` is built on
   `pathfinder_renderer::Renderer` + GL/WebGL resource loading; forking it means
   porting its rasteriser anyway. grafeia uses 7 of its 11 trait methods and 4 of
   its `Context` methods (`goto_page`, `prev_page`, `next_page`, `update_scene`);
   the pan/zoom/clamp logic in its `Context` is ~150 lines worth copying.

## Stages

**Stage 0 — it builds (no rendering change).**
Trim the member list to `core`/`app`/`convert`/`server`/`web`/`gui`; delete the
`tex` object and gate `object/tex.rs` behind an off-by-default `tex` feature
(it needs ReX, which has its own pathfinder problem); drop both `#![feature]`
attributes, replace `GenIter`'s two uses; port `SvgObject` to usvg 0.44; move
`content.rs` to the encoder API with `pathfinder_impl::OutlineEncoder` (which the
`font` repo keeps for exactly this), i.e. `Encoder::GlyphRef = Outline`.
Done when `cargo check -p grafeia_core -p grafeia_convert` is green and
`cargo run --bin demo` still writes a `demo.graf` that the pdf/bin tools read.

**Stage 1 — `vector` gets a vello backend.** `impl_vello` + `impl_vello_cpu`,
`outline_to_bez()` in a shared module, `Paint::Image` → `peniko::Image` brush,
`clip_path` → `push_clip_layer`, `LineStyle` → `kurbo::Stroke`. Done when a test
renders a path with each and compares to a pathfinder raster.

**Stage 2 — headless vello rendering.** A `Page::draw_into(&mut dyn Surface)`
split of the current `render_page` (it already ends in `scene.draw_path`, so this
is mostly moving the scene construction out of `Page`), plus
`bin/render <file.graf> <page> <out.png>` on vello_cpu. This is the first
visible result and the test harness for everything after: compare against a
`pathfinder_view` screenshot of the same page.

**Stage 3 — the shell.** `grafeia_view`: native winit + wgpu + `vello::util`
(`RenderContext`, `Renderer`), wasm `vello_cpu` → `ImageData`/`putImageData`.
`Interactive` ported 1:1 (7 methods), `Context` with the page/zoom/pan/bounds
logic. Stub out: `server` (websocket, no rendering) stays as is; `web` rebuilds
the wasm module; `grafeia-wasm` is a build artifact and just gets rebuilt.

**Stage 4 — SVG objects.** `usvg 0.44` walker: groups (transform + opacity),
paths (`tiny_skia_path` segments → BezPath), fill/stroke/stroke-width, driving
`Surface::draw_path`. ~100 lines in `object/svg.rs`, replacing `BuiltSVG`.
`pathfinder_svg` disappears.

**Stage 5 — delete pathfinder.** `pathfinder_renderer`, `pathfinder_svg`,
`pathfinder_export`, `pathfinder_view` out of every manifest; `vector`'s
`impl_pathfinder` can stay behind its feature (it is 214 lines and documents the
old backend), the commented `[patch]` blocks in `Cargo.toml` go, README's
"WebGL2" note is updated.

## Verification

- Stage 0/2: layout is renderer-independent, so dump `Cache::positions` and
  `word_positions` for `demo.graf` before and after — identical numbers or the
  port broke something.
- Stage 2: raster page 1..3 at 96 dpi with vello_cpu and with pathfinder, then
  compare ink coverage and a coarse grid (`image_analyze`) — catches the y-flip,
  the missing background rect, and curve elevation mistakes, which is most of
  what can go wrong here.
- Stage 3: the emulator/browser check — the page must look the same, cursor and
  selection included.

## ReX

ReX (`grafeia/ReX`, HEAD f969f53 from 2022-02) is the sibling the engine came
from: same `Backend`-trait layout, same `font` crate, `pathfinder_renderer` +
`pathfinder_export` for output. It is ported already, on branch `vello`
(commit abd8ac7), and the port is the template for grafeia's stage 2: it proves
that a `Backend` implementation per renderer is all it takes, and it produced
the conversion helpers grafeia will want.

What the port had to fix before any vello question:

* `font` was taken with `features = ["woff"]` only. Every `ctx.math` access was
  unresolved — the crate did not compile. It needs `["opentype", "woff", "math",
  "cff"]`.
* `font`'s encoder API: `MathFont` is `OpenTypeFont<OutlineEncoder>` now, a
  glyph's shape is `Shape::Simple/Compound/Empty` instead of a ready `Outline`,
  and `downcast_box` is gone. Use `font::parse(data, &mut enc)` /
  `OpenTypeFont::parse(data, &mut OutlineEncoder)`, and
  `font::pathfinder_impl::flatten(font, gid)` where an `Outline` is wanted.
* `tests/` is from a different generation of the crate (`rex::RenderSettings`,
  `render::svg::render_to_string`, `bincode::Infinite`) and does not compile.
  `examples/qc.rs` is the live harness. Left alone; it should be rewritten or
  deleted, not ported.

What replaced what:

| old | new |
| --- | --- |
| `render/scene.rs`, `SceneWrapper` over `pathfinder_renderer::Scene` | `render/svg.rs` (`SvgBackend`), `render/vello_backend.rs` (`VelloBackend`, feature `vello`), `render/cpu.rs` (`CpuBackend`, feature `vello_cpu`) |
| `pathfinder_export` SVG export | `SvgBackend::finish()` — filled `<path>`s, view box measured from what was drawn |
| `pathfinder_color::ColorU` | `peniko`'s `AlphaColor` (via `vello`/`vello_cpu` re-exports) |
| `OutlineStrokeToFill` for debug boxes | kept, in `render/bez.rs` — all three backends draw the same expanded outline, so no backend needs stroke support |

New: `render/bez.rs` (`Outline` → kurbo `BezPath`, `Transform2F` → `Affine`)
and `render/bounds.rs` (`BoundsBackend`, draws nothing, reports the ink box —
a pixmap or texture has to be sized before the first draw, and
`Renderer::size` only knows ascent/descent hints).

Version note: `vello 0.11` resolves to `kurbo 0.13`, so a `kurbo = "0.11.3"`
dependency is a second, incompatible copy — pin `0.13.1` or use
`vello::kurbo`. `vello_cpu 0.3` shares that kurbo here, so one conversion serves
both.

Verified: `cargo check --lib` / `--features vello` / `--features vello_cpu` /
`--examples` in all three configurations; `qc` writes 281 paths (SVG) and a
2280x115 PNG grid for ten sample formulas; `x`, `x + y`, `\frac{1}{2}`,
`\sum_{k=0}^{n} k^2` rasterise with ink where the layout puts it; and the
layout tree for `\frac{1}{2}` is byte-identical to the pre-port one (checked
against f969f53 in a worktree), so the port changed rendering, not metrics.

Two things that look like port damage and are not:

* `Glyph::depth()` is the glyph bbox's `y_min`, and font units have y up, so a
  descender is a *negative* depth (`\frac{1}{2}` → height 12.79, depth -6.4 at
  a 10-unit em). `VBox::build`, `LayoutNode::centered` and the renderer all
  assume that sign; flipping it moves every fraction, radical and script.
* A Display-style fraction's numerator *is* typeset at full size (`Style::Text`
  gives 7.0 at a 10-unit em instead). ReX lays out its own 2020 samples this
  way; the port did not touch metrics — the layout tree is byte-identical.

Still open in ReX, but not blocking grafeia: `samples/*.svg` come from the old
exporter (the new SVG has only paths, no `@font-face`/text) and cannot be
regenerated byte-identically; the fonts `qc` needs are not in the repo
(`tests/out/rex-xits.otf` works); `tests/` should be rewritten or deleted.

### Where the caching goes

Caching of whole scene fragments belongs in grafeia, not in ReX. ReX is a layout
engine: `Backend` stays a handful of `bbox`/`symbol`/`rule`/colour calls that
emit plain colour outlines at relative positions, it never sees a
`vello::Scene`, and it needs no fragment type — nothing has to change in the
`vello` branch to support caching.

The cache sits one layer up, where reuse is actually known. Grafeia's draw
target is today `pathfinder_renderer::scene::Scene` (`core/src/draw.rs`,
`core/src/object/mod.rs`) — rex's deleted `SceneWrapper` idea repeated at the
document level. Stage 2 replaces it with a grafeia-owned fragment:

* `type Fragment = vello::Scene` (behind the vello feature), one per object,
  produced by running the object's `draw()` into a fresh `Scene` instead of
  straight into the page;
* `HashMap<FragmentId, Arc<Scene>>` keyed by (object id, resolved design
  revision), so a repaint appends cached scenes and only re-runs the objects
  whose layout or style changed;
* page assembly is `Scene::append` per fragment — the encoding is already
  renderer-ready, which is the whole point of caching at this level.

Granularity: object level is enough and needs nothing from ReX. Finer
granularity (per line, per inline run) needs ReX to report where a layout node's
contribution starts and ends; if that turns out to matter, it is a `begin`/
`end` mark on `Backend`, not a scene handle.

Two coupling points grafeia has to fix before any of this compiles on `vello`:

* `core/src/object/tex.rs:63` uses `font.downcast()`, which only exists where
  `dyn Font` does (master). On the `vello` branch there is no `dyn Font`: parse
  an `OpenTypeFont<OutlineEncoder>` directly (`OpenTypeFont::parse` /
  `FontVariant::OpenType`), and the `lazy_static` XITS instance will have to
  carry the encoder type.
* `vector` (git, `features = ["impl_pathfinder"]`) is a second path/paint
  abstraction: give it a vello impl, or have grafeia draw kurbo `BezPath`s and
  retire it.

## Open questions

- Does ReX get revived on vello too, or does the `tex` feature stay dead? That
  decides whether `object/tex.rs` is deleted or ported (it uses
  `rex::render::SceneWrapper`, i.e. ReX's own pathfinder backend).
- WebGPU for the wasm shell once it is broadly available, or is CPU-only fine
  for the demo? CPU-only keeps one code path.
- Is `pathfinder_geometry` eventually replaced by kurbo in the engine, or does it
  stay as the coordinate vocabulary? Staying is fine; it only has to change if
  `font`'s `Pen` changes.
