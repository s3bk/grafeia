use serde::{Serialize, Deserialize};
use pathfinder_geometry::{
    vector::Vector2F,
};
use pathfinder_renderer::scene::{Scene};
use crate::*;
use crate::draw::DrawCtx;

pub mod svg;
#[cfg(feature = "tex")]
pub mod tex;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Object {
    Svg(svg::SvgObject),
    /// Needs ReX's layout engine (fine) and a scene to draw a formula into (stage 2).
    #[cfg(feature = "tex")]
    TeX(tex::TeX)
}

#[derive(Serialize, Deserialize)]
#[derive(Debug, Clone)]
pub enum Scale {
    FitWidth,
    FitLineHight,
    FitTextHeight,
    Width(Length),
    Height(Length)
}

pub struct ObjectCtx<'a> {
    pub storage: &'a Storage,
    pub target: &'a Target,
    pub design: &'a Design,
    pub typ: TypeId,
}

impl Object {
    pub fn size(&self, ctx: ObjectCtx) -> (FlexMeasure, Length) {
        match *self {
            Object::Svg(ref svg) => svg.size(ctx),
            #[cfg(feature = "tex")]
            Object::TeX(ref tex) => tex.size(ctx),
        }
    }
    pub fn draw(&self, ctx: ObjectCtx, origin: Vector2F, size: Vector2F, scene: &mut Scene) {
        match *self {
            Object::Svg(ref svg) => svg.draw(ctx, origin, size, scene),
            #[cfg(feature = "tex")]
            Object::TeX(ref tex) => tex.draw(ctx, origin, size, scene),
        }
    }
}
