use serde::{Serialize, Deserialize, Serializer, ser::SerializeTuple};
use pathfinder_color::ColorU;
use pathfinder_content::outline::{Contour, Outline};
use pathfinder_content::stroke::{LineCap, LineJoin, OutlineStrokeToFill, StrokeStyle};
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

/// A stroke, in SVG user units of the picture.
#[derive(Clone, Debug)]
struct Stroke {
    color: ColorU,
    width: f32,
    cap: LineCap,
    join: LineJoin,
    miter_limit: f32
}

/// One `<path>` of the picture: a filled silhouette and/or its outline.
#[derive(Clone, Debug)]
struct Shape {
    outline: Outline,
    fill: Option<ColorU>,
    stroke: Option<Stroke>
}

/// An SVG image, flattened to shapes at parse time.
///
/// `usvg` resolves inheritance and transforms while it parses, so every path is
/// stored in user units with its absolute transform already applied; drawing
/// only has to place and scale the whole picture.
#[derive(Deserialize, Clone)]
#[serde(from="(Scale, Vec<u8>)")]
pub struct SvgObject {
    data: Vec<u8>,
    /// Size of the view box, in SVG user units.
    size: Vector2F,
    shapes: Vec<Shape>,
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
        let mut shapes = Vec::new();
        collect_group(tree.root(), &mut shapes);

        SvgObject {
            scale,
            data,
            size: Vector2F::new(size.width(), size.height()),
            shapes
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
        // `origin` is the baseline of the line the object sits on; an image hangs
        // above it, like the ink of a glyph.
        let tr = Transform2F::from_translation(origin - Vector2F::new(0.0, size.y()))
            * Transform2F::from_scale(Vector2F::splat(scale));

        for shape in &self.shapes {
            let outline = shape.outline.clone().transformed(&tr);
            // Fill first: an SVG paint server paints the fill, then the stroke.
            if let Some(color) = shape.fill {
                let mut fill = outline.clone();
                // fills treat every subpath as closed
                fill.close_all_contours();
                let paint = scene.push_paint(&Paint::from_color(color));
                scene.push_draw_path(DrawPath::new(fill, paint));
            }
            if let Some(ref stroke) = shape.stroke {
                let style = StrokeStyle {
                    line_width: stroke.width * scale,
                    line_cap: stroke.cap,
                    line_join: stroke.join
                };
                let mut widen = OutlineStrokeToFill::new(&outline, style);
                widen.offset();
                let widened = widen.into_outline();
                if !widened.is_empty() {
                    let paint = scene.push_paint(&Paint::from_color(stroke.color));
                    scene.push_draw_path(DrawPath::new(widened, paint));
                }
            }
        }
    }
}

fn collect_group(group: &usvg::Group, shapes: &mut Vec<Shape>) {
    for child in group.children() {
        collect(child, shapes)
    }
}

fn collect(node: &usvg::Node, shapes: &mut Vec<Shape>) {
    match node {
        usvg::Node::Group(ref group) => collect_group(group, shapes),
        usvg::Node::Path(ref path) => {
            let fill = match path.fill() {
                Some(fill) => color_of(fill.paint(), fill.opacity().get()),
                None => None
            };
            let abs_transform = path.abs_transform();
            let transform = transform_of(&abs_transform);
            // A group transform scales the geometry, and with it the stroke width:
            // CSS widths are in the element's own user space.
            let width_scale = (abs_transform.sx * abs_transform.sy
                - abs_transform.kx * abs_transform.ky).abs().sqrt();
            let stroke = path.stroke().and_then(|stroke| {
                let color = color_of(stroke.paint(), stroke.opacity().get())?;
                Some(Stroke {
                    color,
                    width: stroke.width().get() * width_scale,
                    cap: map_line_cap(stroke.linecap()),
                    join: map_line_join(stroke.linejoin(), stroke.miterlimit().get()),
                    miter_limit: stroke.miterlimit().get()
                })
            });
            if fill.is_none() && stroke.is_none() {
                return
            }
            let outline = outline_of(path.data(), &transform);
            if outline.is_empty() {
                return
            }
            shapes.push(Shape { outline, fill, stroke })
        }
        _ => ()
    }
}

fn map_line_cap(cap: usvg::LineCap) -> LineCap {
    match cap {
        usvg::LineCap::Butt => LineCap::Butt,
        usvg::LineCap::Round => LineCap::Round,
        usvg::LineCap::Square => LineCap::Square
    }
}

fn map_line_join(join: usvg::LineJoin, miter_limit: f32) -> LineJoin {
    match join {
        usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoin::Miter(miter_limit),
        usvg::LineJoin::Round => LineJoin::Round,
        usvg::LineJoin::Bevel => LineJoin::Bevel
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
/// every point on the way in. Subpaths are left open unless the path closes
/// them, so strokes do not gain a closing segment; fills close them again.
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
    outline
}
