use pathfinder_renderer::paint::Paint;
use pathfinder_renderer::scene::{DrawPathId, Scene};
use grafeia_core::object::ObjectCtx;
use pathfinder_color::ColorU;
use grafeia_core::*;
use std::io;
use std::fmt::Write;
use std::collections::HashMap;
use pathfinder_content::{outline::{ContourIterFlags, Outline}, segment::SegmentKind};
use pathfinder_geometry::{
    rect::RectF,
    vector::Vector2F,
    transform2d::Transform2F,
};
use grafeia_core::layout::Column;
use grafeia_core::draw::{
    RenderItem,
    WordPart,
    Cache
};


use std::fmt;
/// PDF user space is points — 72 to the inch — and the layout is in
/// millimetres, so every box that reaches the page dictionary has to be
/// scaled.
const PT_PER_MM: f32 = 72.0 / 25.4;

struct PdfPoints(RectF);
impl fmt::Display for PdfPoints {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let ll = self.0.lower_left();
        let ur = self.0.upper_right();
        write!(f, "[{} {} {} {}]",
            ll.x().min(ur.x()) * PT_PER_MM, ll.y().min(ur.y()) * PT_PER_MM,
            ll.x().max(ur.x()) * PT_PER_MM, ll.y().max(ur.y()) * PT_PER_MM)
    }
}

struct PdfRect(RectF);
impl fmt::Display for PdfRect {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // PDF boxes are [llx lly urx ury] with the origin bottom-left; a rect
        // built from a top-down origin reaches here with its corners the other
        // way round, which readers take as a negative box.
        let ll = self.0.lower_left();
        let ur = self.0.upper_right();
        write!(f, "[{} {} {} {}]",
            ll.x().min(ur.x()), ll.y().min(ur.y()),
            ll.x().max(ur.x()), ll.y().max(ur.y()))
    }
}

struct PdfTransform(Transform2F);
impl fmt::Display for PdfTransform {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let m = self.0.matrix;
        let v = self.0.vector;
        write!(f, "{} {} {} {} {} {}", m.m11(), m.m12(), m.m21(), m.m22(), v.x(), v.y())
    }
}

struct Counter<T> {
    inner: T,
    count: u64
}
impl<T> Counter<T> {
    pub fn new(inner: T) -> Counter<T> {
        Counter {
            inner,
            count: 0
        }
    }
    pub fn pos(&self) -> u64 {
        self.count
    }
}
impl<W: io::Write> io::Write for Counter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.inner.write(buf) {
            Ok(n) => {
                self.count += n as u64;
                Ok(n)
            },
            Err(e) => Err(e)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.inner.write_all(buf)?;
        self.count += buf.len() as u64;
        Ok(())
    }
}

/// Represents a PDF internal object
struct PdfObject {
    contents: Vec<u8>,
    is_page: bool,
    is_xobject: bool,
    offset: Option<u64>,
}


struct Stream(String);
impl Stream {
    pub fn new() -> Stream {
        Stream(String::new())
    }
    pub fn move_to(&mut self, p: Vector2F) {
        writeln!(self.0, "{} {} m", p.x(), p.y()).unwrap();
    }

    pub fn line_to(&mut self, p: Vector2F) {
        writeln!(self.0, "{} {} l", p.x(), p.y()).unwrap();
    }

    pub fn cubic_to(&mut self, c1: Vector2F, c2: Vector2F, p: Vector2F) {
        writeln!(self.0, "{} {} {} {} {} {} c", c1.x(), c1.y(), c2.x(), c2.y(), p.x(), p.y()).unwrap();
    }
    pub fn fill(&mut self) {
        writeln!(self.0, "f").unwrap();
    }

    pub fn set_fill_color(&mut self, color: ColorU) {
        let to = |c: u8| c as f32 / 255.0;
        writeln!(self.0, "{} {} {} rg", to(color.r), to(color.g), to(color.b)).unwrap();
    }

    pub fn close(&mut self) {
        writeln!(self.0, "h").unwrap();
    }

    pub fn use_define(&mut self, n: usize, tr: Transform2F) {
        writeln!(self.0, "q {} cm /x{} Do Q", PdfTransform(tr), n).unwrap();
    }

    pub fn draw_path(&mut self, outline: Outline) {
        for contour in outline.contours() {
            for (segment_index, segment) in contour.iter(ContourIterFlags::empty()).enumerate() {
                if segment_index == 0 {
                    self.move_to(segment.baseline.from());
                }

                match segment.kind {
                    SegmentKind::None => {}
                    SegmentKind::Line => self.line_to(segment.baseline.to()),
                    SegmentKind::Quadratic => {
                        let current = segment.baseline.from();
                        let c = segment.ctrl.from();
                        let p = segment.baseline.to();
                        let c1 = Vector2F::splat(2./3.) * c + Vector2F::splat(1./3.) * current;
                        let c2 = Vector2F::splat(2./3.) * c + Vector2F::splat(1./3.) * p;
                        self.cubic_to(c1, c2, p);
                    }
                    SegmentKind::Cubic => self.cubic_to(
                        segment.ctrl.from(),
                        segment.ctrl.to(),
                        segment.baseline.to()
                    )
                }
            }

            if contour.is_closed() {
                self.close();
            }
        }
    }
}
/// The top-level struct that represents a (partially) in-memory PDF file
pub struct Pdf {
    objects: Vec<PdfObject>,
    defines: HashMap<RenderItem, usize>,
    page_height: f32,
}
impl Pdf {
    /// Create a new blank PDF document
    #[inline]
    pub fn new() -> Self {
        Self {
            // 1 = catalog, 2 = page tree, 3 = the shared /Resources dictionary.
            // Object 3 is reserved up front so every page can name it before the
            // xobjects themselves exist.
            objects: vec![
                PdfObject {
                    contents: Vec::new(),
                    is_page: false,
                    is_xobject: false,
                    offset: None,
                },
                PdfObject {
                    contents: Vec::new(),
                    is_page: false,
                    is_xobject: false,
                    offset: None,
                },
                PdfObject {
                    contents: Vec::new(),
                    is_page: false,
                    is_xobject: false,
                    offset: None,
                },
            ],
            defines: HashMap::new(),
            page_height: 0.0,
        }
    }
    fn defined(&mut self, key: RenderItem, f: impl FnOnce() -> Outline) -> usize {
        if let Some(&n) = self.defines.get(&key) {
            return n;
        }
        let outline = f();
        let bbox = outline.bounds();
        let mut stream = Stream::new();
        stream.draw_path(outline);
        stream.fill();

        let mut dict = String::new();
        writeln!(dict, "<< /Type XObject");
        writeln!(dict, "/Subtype /Form");
        writeln!(dict, "/BBox {}", PdfRect(bbox));
        writeln!(dict, "/Length {} >>\nstream\n{}endstream\n", stream.0.len(), stream.0);

        let n = self.add_object(dict.into_bytes(), false, true);
        self.defines.insert(key, n);
        n
    }
    pub fn render_page(&mut self, cache: &Cache, storage: &Storage, target: &Target, design: &Design, column: Column) {
        let content_box: RectF = target.content_box.into();
        let media_box: RectF = target.media_box.into();
        self.page_height = media_box.height() * PT_PER_MM;
        let mut stream = Stream::new();
        // Millimetres into points, and top-down layout coordinates into PDF's
        // bottom-left origin: scale, then flip y about the top edge.
        writeln!(stream.0, "{} 0 0 -{} 0 {} cm", PT_PER_MM, PT_PER_MM, self.page_height);

        for (y, line) in column {
            for (x, size, item, tag) in line {
                let size: Vector2F = size.into();
                let p = content_box.origin() + Vector2F::new(x.value as f32, y.value as f32);
                let rect = RectF::new(p - Vector2F::new(0.0, size.y()), size);
                match item {
                    RenderItem::Word(key, part, font) => {
                        let n = self.defined(item, || {
                            let layout = cache.word_layout_cache.get(&(font, key, part)).unwrap();
                            let font = storage.get_font_face(font.font_face);
                            layout.render(font, Transform2F::default())
                        });
                        stream.use_define(n, Transform2F::from_translation(p));
                    }
                    RenderItem::Margin(key, font, hang) => {
                        // A margin number: the same glyphs as a word, placed
                        // `hang` to the left of where its line begins.
                        let n = self.defined(item, || {
                            let layout = cache.word_layout_cache.get(&(font, key, WordPart::Full)).unwrap();
                            let font = storage.get_font_face(font.font_face);
                            layout.render(font, Transform2F::default())
                        });
                        let at = p - Vector2F::new(hang.value as f32, 0.0);
                        // Same 50% grey as the raster path: the form carries
                        // only the outline, so the colour is set here.
                        stream.set_fill_color(ColorU::new(128, 128, 128, 255));
                        stream.use_define(n, Transform2F::from_translation(at));
                        stream.set_fill_color(ColorU::new(0, 0, 0, 255));
                    }
                    RenderItem::Symbol(key, font) => {
                        let n = self.defined(item, || {
                            let layout = cache.symbol_layout_cache.get(&(font, key)).unwrap();
                            let font = storage.get_font_face(font.font_face);
                            layout.render(font, Transform2F::default())
                        });
                        stream.use_define(n, Transform2F::from_translation(p));
                    }
                    RenderItem::Object(key) => {
                        // Objects draw themselves into a scene; emit those paths
                        // with their own colour, placed at the object's origin.
                        let object = storage.get_object(key);
                        let typ = storage.get_weave(tag.seq()).typ();
                        let mut scene = Scene::new();
                        object.draw(
                            ObjectCtx { storage, target, design, typ },
                            Vector2F::zero(),
                            size,
                            &mut scene
                        );
                        writeln!(stream.0, "q 1 0 0 1 {} {} cm", p.x(), p.y()).unwrap();
                        for n in 0 .. scene.draw_path_count() {
                            let draw_path = scene.get_draw_path(DrawPathId(n)).clone();
                            let color = scene.get_paint(draw_path.paint).base_color();
                            stream.set_fill_color(color);
                            stream.draw_path(draw_path.outline);
                            stream.fill();
                        }
                        writeln!(stream.0, "Q").unwrap();
                    }
                    RenderItem::Empty => {}
                };
            }
        }
        
        let page_stream = format!("<< /Length {} >>\nstream\n{}endstream\n", stream.0.len(), stream.0).into_bytes();

        // Create the stream object for this page
        let stream_object_id = self.add_object(page_stream, false, false);

        // Create the page object, which describes settings for the whole page
        let page_object = format!("\
<< /Type /Page
/Parent 2 0 R
/MediaBox {}
/TrimBox {}
/Resources 3 0 R
/Contents {} 0 R
>>\n",
        PdfPoints(target.media_box.into()),
                PdfPoints(target.trim_box.into()),
                stream_object_id
        ).into_bytes();
        self.add_object(page_object, true, false);
    }

    fn add_object(&mut self, data: Vec<u8>, is_page: bool, is_xobject: bool) -> usize {
        self.objects.push(PdfObject {
            contents: data,
            is_page,
            is_xobject,
            offset: None,
        });
        self.objects.len()
    }

    /// Write the in-memory PDF representation to disk
    pub fn write_to(&mut self, writer: impl io::Write) -> io::Result<()> {
        let mut out = Counter::new(writer);
        use std::io::Write;
        out.write_all(b"%PDF-1.7\n%\xB5\xED\xAE\xFB\n")?;

        let mut xobjects = String::from("<< /XObject <<");
        for n in self.defines.values() {
            writeln!(xobjects, "/x{} {} 0 R", n, n);
        }
        writeln!(xobjects, ">> >>");
        self.objects[2].contents = xobjects.into_bytes();
        let xobjects_nr = 3usize;

        // Write out each object
        for (idx, obj) in self.objects.iter_mut().enumerate().skip(2) {
            obj.offset = Some(out.pos());
            write!(out, "{} 0 obj\n", idx+1)?;
            out.write_all(&obj.contents)?;
            out.write_all(b"endobj\n")?;
        }

        // Write out the page tree object
        self.objects[1].offset = Some(out.pos());
        out.write_all(b"2 0 obj\n")?;
        out.write_all(b"<< /Type /Pages\n")?;
        writeln!(out, "/Resources {} 0 R", xobjects_nr);
        write!(out,
            "/Count {}\n",
            self.objects.iter().filter(|o| o.is_page).count()
        )?;
        out.write_all(b"/Kids [")?;
        for (idx, _obj) in self.objects.iter().enumerate().filter(|&(_, obj)| obj.is_page) {
            write!(out, "{} 0 R ", idx + 1)?;
        }
        out.write_all(b"] >>\nendobj\n")?;

        // Write out the catalog dictionary object
        self.objects[0].offset = Some(out.pos());
        out.write_all(b"1 0 obj\n<< /Type /Catalog\n/Pages 2 0 R >>\nendobj\n")?;

        // Write the cross-reference table
        let startxref = out.pos();
        out.write_all(b"xref\n")?;
        write!(out, "0 {}\n", self.objects.len() + 1)?;
        out.write_all(b"0000000000 65535 f \n")?;

        for obj in &self.objects {
            write!(out, "{:010} 00000 n \n", obj.offset.unwrap())?;
        }

        // Write the document trailer
        out.write_all(b"trailer\n")?;
        write!(out, "<< /Size {}\n", self.objects.len() + 1)?;
        out.write_all(b"/Root 1 0 R >>\n")?;

        // Write the offset to the xref table
        write!(out, "startxref\n{}\n", startxref)?;

        // Write the PDF EOF
        out.write_all(b"%%EOF")?;

        Ok(())
    }
}


fn main() {
    use std::fs::File;
    env_logger::init();

    let mut args = std::env::args().skip(1);
    let input = File::open(args.next().expect("no input given")).expect("can't open input file");
    let output = File::create(args.next().expect("no output file given")).expect("can't create output file");
    
    let state = State::load(input).unwrap();
    let mut cache = Cache::new();

    let layout = cache.layout(&state.storage, &state.design, &state.target, state.root);

    let mut pdf = Pdf::new();
    for column in cache.columns.as_ref().unwrap().columns() {
        pdf.render_page(&cache, &state.storage, &state.target, &state.design, column);
    }
    pdf.write_to(output);
}
