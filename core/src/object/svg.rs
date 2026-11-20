use serde::{Serialize, Deserialize, Serializer, ser::SerializeTuple};
use pathfinder_color::ColorU;
use pathfinder_content::outline::{Contour, Outline};
use pathfinder_geometry::{
    vector::Vector2F,
    transform2d::Transform2F
};
use pathfinder_renderer::{
    paint::Paint,
    scene::{Scene, DrawPath}
};
use crate::*;
use std::hash::{Hash, Hasher};
use std::fmt;

/// An SVG image, flattened to filled outlines at parse time.
///
/// `usvg` resolves transforms while it parses, so every path is stored in user
/// units with its absolute transform already applied; drawing only has to place
/// and scale the whole picture.
#[derive(Deserialize, Clone)]
#[serde(from="(Scale, Vec<u8>)")]
pub struct SvgObject {
    data: Vec<u8>,
    /// Size of the view box, in SVG user units.
    size: Vector2F,
    paths: Vec<(Outline, ColorU)>,
    scale: Scale
}
impl fmt::Debug for SvgObject {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SVG")
    }
}
impl Hash for SvgObject {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.data.hash(state)
    }
}
impl PartialEq for SvgObject {
    fn eq(&self, other: &Self) -> bool {
        self.data.eq(&other.data)
    }
}
impl Eq for SvgObject {}

impl SvgObject {
    pub fn new(scale: Scale, data: Vec<u8>) -> Self {
        use usvg::{Tree, Options};
        let tree = Tree::from_data(&data, &Options::default()).unwrap();
        let size = tree.size();
        let mut paths = Vec::new();
        collect_group(tree.root(), &mut paths);

        SvgObject {
            scale,
            data,
            size: Vector2F::new(size.width(), size.height()),
            paths
        }
    }
}
impl From<(Scale, Vec<u8>)> for SvgObject {
    fn from((scale, data): (Scale, Vec<u8>)) -> Self {
        Self::new(scale, data)
    }
}
impl Serialize for SvgObject {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut t = serializer.serialize_tuple(2)?;
        t.serialize_element(&self.scale)?;
        t.serialize_element(&self.data)?;
        t.end()
    }
}
impl SvgObject {
    pub fn size(&self, ctx: ObjectCtx) -> (FlexMeasure, Length) {
        let type_design = ctx.design.get_type_or_default(ctx.typ);
        let svg_size = self.size;
        let (w, h) = match self.scale {
            Scale::FitWidth => (Some(ctx.target.content_box.width), None),
            Scale::FitLineHight => (None, Some(type_design.line_height)),
            Scale::FitTextHeight => (None, Some(type_design.font.size)),
            Scale::Width(w) => (Some(w), None),
            Scale::Height(h) => (None, Some(h))
        };
        let (w, h) = match (w, h) {
            (Some(w), None) => (w, w * (svg_size.y() / svg_size.x())),
            (None, Some(h)) => (h * (svg_size.x() / svg_size.y()), h),
            _ => unreachable!()
        };
        (FlexMeasure::fixed(w), h)
    }

    pub fn draw(&self, _: ObjectCtx, origin: Vector2F, size: Vector2F, scene: &mut Scene) {
        let svg_size = self.size;
        let scale = size.x() / svg_size.x();
        let tr = Transform2F::from_translation(origin)
            * Transform2F::from_scale(Vector2F::splat(scale));

        for &(ref outline, color) in &self.paths {
            let paint = scene.push_paint(&Paint::from_color(color));
            scene.push_draw_path(DrawPath::new(outline.clone().transformed(&tr), paint));
        }
    }
}

fn collect_group(group: &usvg::Group, paths: &mut Vec<(Outline, ColorU)>) {
    for child in group.children() {
        collect(child, paths)
    }
}

fn collect(node: &usvg::Node, paths: &mut Vec<(Outline, ColorU)>) {
    match node {
        usvg::Node::Group(ref group) => collect_group(group, paths),
        usvg::Node::Path(ref path) => {
            let color = match path.fill() {
                Some(fill) => color_of(fill.paint(), fill.opacity().get()),
                None => None
            };
            if let Some(color) = color {
                let outline = outline_of(path.data(), &transform_of(&path.abs_transform()));
                if !outline.is_empty() {
                    paths.push((outline, color))
                }
            }
        }
        _ => ()
    }
}

/// usvg 0.44 paints colors as RGB; opacity is carried separately.
fn color_of(paint: &usvg::Paint, opacity: f32) -> Option<ColorU> {
    let alpha = (opacity.max(0.0).min(1.0) * 255.0).round() as u8;
    match paint {
        usvg::Paint::Color(color) =>
            Some(ColorU::new(color.red, color.green, color.blue, alpha)),
        // Gradients and patterns are not part of the scene model yet.
        _ => None
    }
}

/// `usvg::Transform` is the `tiny-skia` affine: `sx ky kx sy tx ty` with column
/// vectors, while `Transform2F::row_major` wants a row-major `[m11 m12 m13; m21 m22 m23]`,
/// so the translation goes in slots 3 and 6 and x/y scale on the diagonal.
fn transform_of(transform: &usvg::Transform) -> Transform2F {
    Transform2F::row_major(transform.sx, transform.kx, transform.tx,
                           transform.ky, transform.sy, transform.ty)
}

/// Converts a `tiny-skia` path to a pathfinder outline, applying `transform` to
/// every point on the way in.
fn outline_of(path: &usvg::tiny_skia_path::Path, transform: &Transform2F) -> Outline {
    use usvg::tiny_skia_path::PathSegment;

    let tr = |p: usvg::tiny_skia_path::Point| -> Vector2F {
        *transform * Vector2F::new(p.x, p.y)
    };

    let mut outline = Outline::new();
    let mut contour = Contour::new();
    let mut started = false;

    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                if started {
                    outline.push_contour(std::mem::replace(&mut contour, Contour::new()));
                }
                contour.push_endpoint(tr(p));
                started = true;
            }
            PathSegment::LineTo(p) => {
                contour.push_endpoint(tr(p));
            }
            PathSegment::QuadTo(ctrl, p) => {
                contour.push_quadratic(tr(ctrl), tr(p));
            }
            PathSegment::CubicTo(ctrl0, ctrl1, p) => {
                contour.push_cubic(tr(ctrl0), tr(ctrl1), tr(p));
            }
            PathSegment::Close => contour.close()
        }
    }
    if started {
        outline.push_contour(contour);
    }
    // fills treat every subpath as closed
    outline.close_all_contours();
    outline
}
