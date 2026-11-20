use serde::{Serialize, Deserialize};
use pathfinder_geometry::{
    vector::Vector2F,
    rect::RectF,
    transform2d::Transform2F
};
use pathfinder_color::ColorU;
use pathfinder_content::outline::Outline;
use pathfinder_renderer::{
    paint::Paint,
    scene::{Scene, DrawPath}
};
use crate::*;
use crate::object::DrawCtx;
use std::fmt;

pub use rex::layout::{
    Style as TeXStyle,
};
use rex::{
    Renderer,
    layout::{Layout, LayoutSettings},
};
use rex::render::{Backend, Cursor, MathFont};
use rex::font::FontContext;
use rex::dimensions::{self, Px};
use rex::parser::color::RGBA;
use font::{Font, GlyphId, pathfinder_impl::flatten};

#[derive(Serialize, Deserialize, Clone)]
pub struct TeX {
    tex: String,
    style: TeXStyle,
    font: FontId
}
impl fmt::Debug for TeX {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.tex)
    }
}
fn cvt(v: dimensions::Length<Px>) -> Length {
    Length::mm((v / Px) as f32)
}

/// Draws a ReX layout into a pathfinder [`Scene`].
///
/// This is what `rex::render::SvgBackend` does, minus serialisation: every glyph
/// becomes a filled outline in the scene, with the transform chain the old
/// `SceneWrapper` used (`pos`, the y flip into ReX's y-down space, the font matrix,
/// then the object's own placement).
struct SceneBackend<'a> {
    scene: &'a mut Scene,
    transform: Transform2F,
    color: ColorU,
    color_stack: Vec<ColorU>
}
impl<'a> SceneBackend<'a> {
    fn new(scene: &'a mut Scene, transform: Transform2F) -> Self {
        SceneBackend {
            scene,
            transform,
            color: ColorU::new(0, 0, 0, 255),
            color_stack: Vec::new()
        }
    }
    fn fill(&mut self, outline: &Outline, transform: Transform2F) {
        let outline = outline.clone().transformed(&(self.transform * transform));
        if outline.is_empty() { return }
        let paint = self.scene.push_paint(&Paint::from_color(self.color));
        self.scene.push_draw_path(DrawPath::new(outline, paint));
    }
}
impl<'a> Backend for SceneBackend<'a> {
    fn symbol(&mut self, pos: Cursor, gid: u16, scale: f64, ctx: &MathFont) {
        let outline = match flatten(ctx, GlyphId(gid as u32)) {
            Some(outline) => outline,
            None => return
        };
        let transform = Transform2F::from_translation(Vector2F::new(pos.x as f32, pos.y as f32))
            * Transform2F::from_scale(Vector2F::new(scale as f32, -scale as f32))
            * ctx.font_matrix();
        self.fill(&outline, transform);
    }
    fn rule(&mut self, pos: Cursor, width: f64, height: f64) {
        let outline = Outline::from_rect(RectF::new(
            Vector2F::new(pos.x as f32, pos.y as f32),
            Vector2F::new(width as f32, height as f32)
        ));
        self.fill(&outline, Transform2F::default());
    }
    fn begin_color(&mut self, RGBA(r, g, b, a): RGBA) {
        self.color_stack.push(self.color);
        self.color = ColorU::new(r, g, b, a);
    }
    fn end_color(&mut self) {
        self.color = self.color_stack.pop().unwrap_or(ColorU::new(0, 0, 0, 255));
    }
}

impl TeX {
    pub fn display(tex: impl Into<String>, font: FontId) -> Self {
        TeX::new(tex, TeXStyle::Display, font)
    }
    pub fn text(tex: impl Into<String>, font: FontId) -> Self {
        TeX::new(tex, TeXStyle::Text, font)
    }
    pub fn new(tex: impl Into<String>, style: TeXStyle, font: FontId) -> Self {
        TeX {
            tex: tex.into(),
            style,
            font
        }
    }
    fn build<T>(&self, ctx: ObjectCtx, f: impl FnOnce(Layout<'_>, Renderer, &TypeDesign) -> T) -> T {
        let type_design = ctx.design.get_type_or_default(ctx.typ);
        let font = ctx.storage.get_font_face(self.font);
        let font_ctx = FontContext::new(&font);
        let renderer = Renderer::new();
        let layout_settings = LayoutSettings::new(&font_ctx, type_design.font.size.value as f64, self.style);
        let layout = renderer.layout(&self.tex, layout_settings).unwrap();
        f(layout, renderer, type_design)
    }
    pub fn size(&self, ctx: ObjectCtx) -> (FlexMeasure, Length) {
        self.build(ctx, |layout, renderer, type_design| {
            let _ = renderer.size(&layout);
            // Left and right padding
            let width = layout.width;
            // Top and bot padding
            let height = match self.style {
                TeXStyle::Display | TeXStyle::DisplayCramped => cvt(layout.height - layout.depth),
                _ => type_design.line_height
            };

            (FlexMeasure::fixed(cvt(width)), height)
        })
    }
    pub fn draw(&self, ctx: ObjectCtx, origin: Vector2F, _size: Vector2F, scene: &mut Scene) {
        self.build(ctx, |layout, renderer, type_design| {
            let _ = type_design;
            let y_off = match self.style {
                TeXStyle::Display | TeXStyle::DisplayCramped => layout.depth / Px,
                _ => 0.0
            };
            let transform = Transform2F::from_translation(origin + Vector2F::new(0.0, y_off as f32));
            let mut backend = SceneBackend::new(scene, transform);
            renderer.render(&layout, &mut backend);
        })
    }
}
