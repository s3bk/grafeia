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

## Open questions

- Does ReX get revived on vello too, or does the `tex` feature stay dead? That
  decides whether `object/tex.rs` is deleted or ported (it uses
  `rex::render::SceneWrapper`, i.e. ReX's own pathfinder backend).
- WebGPU for the wasm shell once it is broadly available, or is CPU-only fine
  for the demo? CPU-only keeps one code path.
- Is `pathfinder_geometry` eventually replaced by kurbo in the engine, or does it
  stay as the coordinate vocabulary? Staying is fine; it only has to change if
  `font`'s `Pen` changes.
