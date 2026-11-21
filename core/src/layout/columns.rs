use super::{Entry, StreamVec, FlexMeasure, ItemMeasure, Glue};
//use layout::style::{Style};
use crate::units::{Length, Size};
use std::fmt::{self, Debug};
use crate::content::{Font, Tag};
use crate::draw::RenderItem;

#[derive(Copy, Clone, Debug, Default)]
struct LineBreak {
    prev:   usize, // index to previous line-break
    path:   u64, // one bit for each branch taken (1) or not (0)
    factor: f32,
    score:  f32,
    height: Length,
    width:  Length,
    indent: Length,
}

#[derive(Copy, Clone, Debug, Default)]
struct ColumnBreak {
    prev:   usize, // index to previous column-break
    score:  f32,
}
    
#[derive(Copy, Clone, Debug, Default)]
struct Break {
    line:   LineBreak,
    column: Option<ColumnBreak>
}

#[derive(Debug)]
pub struct ParagraphStyle {
    pub font: Font,
    pub leading: f32,
    pub par_indent: f32
}

pub struct ParagraphLayout {
    items:      Vec<Entry>,
    nodes:      Vec<Option<LineBreak>>,
    last:       usize
}
pub struct ColumnLayout {
    para:       ParagraphLayout,
    nodes_col:  Vec<Option<ColumnBreak>>,
    height:     Length,
    // how many lines of its own paragraph are there before / after a break
    // point: one of them alone at a column edge is a widow or an orphan.
    before:     Vec<u16>,
    after:      Vec<u16>,
    // the penalty carried by the break that opened a break point's paragraph:
    // a heading passes its own "keep with what follows" on to the paragraph
    // underneath it, which then needs two lines at the foot of the column.
    guard:      Vec<f32>,
    // the same for the paragraph that ends at a break point
    close_guard: Vec<f32>,
    // lines in the paragraph a break point belongs to
    total:      Vec<u16>,
}
impl Debug for ColumnLayout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ColumnLayout")
    }
}
impl Debug for ParagraphLayout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ParagraphLayout")
    }
}

struct Context {
    measure:    FlexMeasure,
    overflow:   FlexMeasure, // how much to overflow into the margin
    height:     Length,
    indent:     Length,
    width:      Length,
    path:       u64,    // one bit for each branch on this line
    begin:      usize,  // begin of line or branch
    pos:        usize,  // calculation starts here
    score:      f32,    // score at pos
    branches:   u8,     // number of branches so far (<= 64)
}
impl Context {
    fn new(start: usize, score: f32, indent: Length, width: Length) -> Context {
        Context {
            measure:    FlexMeasure::zero(),
            overflow:   FlexMeasure::zero(),
            height:     Length::zero(),
            indent,
            width,
            path:       0,
            begin:      start,
            pos:        start,
            branches:   0,
            score:      score,
        }
    }
    fn add_item(&mut self, item: ItemMeasure, is_first: bool) {
        if is_first {
            self.measure -= item.left;
        }
        self.measure += item.content;
        self.overflow = item.right;

        // TODO: vertical alignment
        self.height = self.height.max(item.height);
    }
    fn add_space(&mut self, s: FlexMeasure) {
        self.measure += s;
        self.overflow = FlexMeasure::zero();
    }
    fn line(&self) -> FlexMeasure {
        self.measure - self.overflow
    }
    fn fill(&mut self, width: Length) {
        self.measure.extend(width);
        self.overflow = FlexMeasure::zero();
    }
}

impl ParagraphLayout {
    pub fn new(items: StreamVec, width: Length) -> ParagraphLayout {
        let limit = items.0.len();
        let mut nodes = vec![None; limit+1];
        nodes[0] = Some(LineBreak {
            width,
            .. LineBreak::default()
        });

        let mut layout = ParagraphLayout {
            nodes,
            items: items.0,
            last: 0
        };
        layout.run();
        layout
    }
    fn run(&mut self) {
        let mut last = 0;
        for start in 0 .. self.items.len() {
            match self.nodes[start] {
                Some(b) => {
                    last = self.complete_line(
                        start,
                        Context::new(start, b.score, b.indent, b.width)
                    );
                },
                None => {}
            }
        }

        if self.nodes[last].is_none() {
            for i in 0 .. last {
                println!("{:3} {:?}", i, self.items[i]);
                if let Some(b) = self.nodes[i] {
                    println!("     {:?}", b);
                }
            }
        }

        self.last = last;
    }

    fn complete_line(&mut self, start: usize, mut c: Context) -> usize {
        let mut last = c.begin;
        
        while c.pos < self.items.len() {
            let n = c.pos;
            let is_first = start == n;
            match self.items[n] {
                Entry::Item(m, _, _) => c.add_item(m, is_first),
                Entry::Space(s, Some(penalty), _) => {
                    // breaking case:
                    // width is not added yet!
                    self.maybe_update(&c, n+1, penalty);
                    last = n+1;
                    
                    // add width now.
                    c.add_space(s);
                },
                Entry::Space(s, None, _) => {
                    c.add_space(s);
                }

                Entry::Linebreak(fill, _, _) => {
                    if fill {
                        c.fill(c.width);
                    }
                    
                    self.maybe_update(&c, n+1, 0.0);
                    last = n+1;
                    break;
                },

                Entry::Column => {
                    self.maybe_update(&c, n+1, 0.0);
                    last = n+1;
                    break;
                }

                Entry::SetWidth(indent, width) => {
                    c.indent = indent;
                    c.width = width;
                }

                Entry::BranchEntry(len) => {
                    // b
                    let b_last = self.complete_line(
                        start,
                        Context {
                            pos:        n + 1,
                            path:       c.path | (1 << c.branches),
                            branches:   c.branches + 1,
                            ..          c
                        }
                    );
                    if b_last > last {
                        last = b_last;
                    }
                    
                    // a follows here
                    c.pos += len;
                    c.branches += 1;
                },
                Entry::BranchExit(skip) => {
                    c.pos += skip;
                }
            }
            
            if c.measure.shrink > c.width {
                break; // too full
            }
            
            c.pos += 1;
        }
        
        last
    }

    fn maybe_update(&mut self, c: &Context, n: usize, penalty: f32) {
        let (factor, score) = match c.line().factor(c.width) {
            Some(factor) => (factor, -factor * factor),
            None => (1.0, -1000.)
        };

        let break_score = c.score + score + penalty;
        let break_point = LineBreak {
            factor: factor,
            score:  break_score,
            prev:   c.begin,
            path:   c.path,
            height: c.height,
            width:  c.width,
            indent: c.indent,
        };
        self.nodes[n] = Some(match self.nodes[n] {
            Some(line) if break_score <= line.score => line,
            _ => break_point
        });
    }
    pub fn lines(&self) -> Column {
        Column::new(0, self.last, self)
    }
}
impl ColumnLayout {
    pub fn new(items: StreamVec, width: Length, height: Length) -> ColumnLayout {
        let limit = items.0.len();
        debug!("{} items", limit);
        let mut nodes = vec![None; limit+1];
        let mut nodes_col = vec![None; limit+1];

        nodes[0] = Some(LineBreak {
            width,
            .. LineBreak::default()
        });
        nodes_col[0] = Some(ColumnBreak::default());

        let len = items.0.len();
        let mut layout = ColumnLayout {
            para: ParagraphLayout {
                nodes,
                items: items.0,
                last: 0
            },
            nodes_col,
            height,
            before: vec![1; len + 1],
            after: vec![1; len + 1],
            guard: vec![0.0; len + 1],
            close_guard: vec![0.0; len + 1],
            total: vec![1; len + 1],
        };
        layout.run();
        layout
    }
    pub fn columns(self) -> Columns {
        Columns::new(self)
    }
    fn run(&mut self) {
        // Break every line first: the column pass needs to know, for any break
        // point, how many lines of its paragraph lie before and after it.
        let mut last = 0;
        for start in 0 .. self.para.items.len() {
            match self.para.nodes[start] {
                Some(b) => {
                    last = self.para.complete_line(
                        start,
                        Context::new(start, b.score, b.indent, b.width)
                    );
                },
                None => {}
            }
        }
        self.compute_paragraph_lines();

        // Every column break is looked for twice: first with the widow,
        // orphan and heading rules as hard requirements, and only where that
        // leaves nothing at all, with them as penalties instead.
        for strict in [true, false] {
            for start in 0 ..= self.para.items.len() {
                if self.para.nodes[start].is_none() || self.nodes_col[start].is_some() {
                    continue;
                }
                self.compute_column(start, strict, start == last);
            }
        }

        if self.nodes_col[last].is_none() {
            for i in 0 .. last {
                println!("{:3} {:?}", i, self.para.items[i]);
                if let Some(b) = self.para.nodes[i] {
                    println!("     {:?}", b);
                }
                if let Some(l) = self.nodes_col[i] {
                    println!("     {:?}", l);
                }
            }
        }

        self.para.last = last;
    }

    fn num_lines_penalty(&self, n: usize) -> f32 {
        match n {
           // 0 => -100.0,
            // one line of a paragraph alone at a column edge: the last line
            // must not wrap on its own, and neither may the first one stay
            // behind on its own.
            1 => -500.0,
            _ => 0.0
        }
    }
    /// How much an underfull column costs. Calibrated against the widow and
    /// orphan penalties: leaving a column half empty costs about as much as
    /// leaving a single line alone, so the layout trades one for the other
    /// only when the hole would be enormous.
    fn fill_penalty(&self, fill: Length) -> f32 {
        -2000.0 * ((self.height - fill) / self.height)
    }

    /// For every break point: how many lines of the paragraph it belongs to
    /// are there before it and after it. Both ends of a column need two of
    /// them, never one.
    fn compute_paragraph_lines(&mut self) {
        let len = self.para.items.len();
        let mut end_of_line = vec![None; len + 1];
        for m in 0 ..= len {
            if let Some(b) = self.para.nodes[m] {
                if b.prev < m {
                    end_of_line[b.prev] = Some(m);
                }
            }
        }

        // A line continues in its paragraph after a word space, and after a
        // hyphenation break — the latter carries no column break, so it is not
        // the end of anything.
        let continues = |items: &[Entry], m: usize| m > 0 && matches!(
            items[m - 1],
            Entry::Space(..) | Entry::Linebreak(_, _, None)
        );

        let mut before = vec![1u16; len + 1];
        for n in 1 ..= len {
            if let Some(b) = self.para.nodes[n] {
                if continues(&self.para.items, n) {
                    before[n] = before[b.prev].saturating_add(1);
                }
            }
        }

        let mut after = vec![1u16; len + 1];
        for n in (1 ..= len).rev() {
            if self.para.nodes[n].is_none() {
                continue;
            }
            after[n] = match end_of_line[n] {
                Some(m) if continues(&self.para.items, m) => after[m].saturating_add(1),
                _ => 1,
            };
        }

        let mut guard = vec![0.0f32; len + 1];
        let mut close_guard = vec![0.0f32; len + 1];
        let mut total = vec![1u16; len + 1];
        let mut opened_by = 0.0f32;
        let mut start = 0usize;
        for n in 1 ..= len {
            total[n] = after[start].max(1);
            if let Entry::Linebreak(_, _, Some(column_break)) = self.para.items[n - 1] {
                // the paragraph that ends here was opened by this
                close_guard[n] = opened_by;
                opened_by = column_break.min(0.0);
                start = n;
            }
            guard[n] = opened_by;
        }

        self.before = before;
        self.after = after;
        self.guard = guard;
        self.close_guard = close_guard;
        self.total = total;
    }

    /// Whether a column that starts at this break point leaves a single line
    /// of a paragraph alone at one of its edges, or a heading with a single
    /// line under it.
    fn leaves_one_line(&self, last: usize) -> bool {
        if last == 0 {
            return false;
        }
        match self.para.items[last - 1] {
            // one line of the paragraph on either side of the break, or the
            // single line of a paragraph that opened under a heading
            Entry::Space(..) => self.before[last] == 1
                || self.after[last] == 1
                || (self.guard[last] < 0.0 && self.before[last] == 1),
            // a heading left with a one-line paragraph under it
            Entry::Linebreak(..) => self.total[last] == 1 && self.close_guard[last] < 0.0,
            _ => false,
        }
    }

    fn compute_column(&mut self, n: usize, strict: bool, is_last: bool) -> bool {
        //                                        measure:
        let mut height = Length::zero();
        let mut last = n;
        let mut found = false;

        // walk backwards
        loop {
            let last_node = self.para.nodes[last].unwrap();

            let penalty = if last > 0 {
                let penalty = match self.para.items[last-1] {
                    Entry::Linebreak(_, skip, col_break) => {
                        height += skip;
                        col_break
                    },
                    Entry::Space(_, _, col_break) => col_break,
                    Entry::Column => Some(Glue::PAGE_BREAK),
                    ref e => panic!("found: {:?}", e)
                };

                height += last_node.height;

                if height > self.height {
                    break;
                }

                penalty
            } else {
                Some(0.0)
            };

            if strict && self.leaves_one_line(last) {
                if last == 0 {
                    break;
                }
                last = last_node.prev;
                continue;
            }

            if let (Some(penalty), Some(column)) = (penalty, self.nodes_col[last]) {
                let mut score = column.score + penalty;

                // A paragraph may not leave a single line alone at a column
                // edge: one line at the foot of the page (orphan) and one at
                // the top of the next (widow) both break the reading.
                if last > 0 {
                    match self.para.items[last-1] {
                        Entry::Space(..) => {
                            score += self.num_lines_penalty(self.before[last] as usize)
                                + self.num_lines_penalty(self.after[last] as usize);

                            if self.before[last] == 1 {
                                // the paragraph opened under a heading that
                                // keeps with what follows: one line of it is
                                // not enough company
                                score += self.guard[last];
                            }
                        }
                        Entry::Linebreak(..) => {
                            // a heading may not be left with a one-line
                            // paragraph under it either
                            if self.total[last] == 1 {
                                score += self.close_guard[last];
                            }
                        }
                        _ => {}
                    }
                }

                if !is_last {
                    score += self.fill_penalty(height);
                }

                match self.nodes_col[n] {
                    Some(column) if column.score > score => {},
                    _ => {
                        self.nodes_col[n] = Some(ColumnBreak {
                            prev: last,
                            score: score
                        });

                        found = true;
                    }
                }
            }

            if last == 0 {
                break;
            }
            last = last_node.prev;
        }

        found
    }
}

#[derive(Debug)]
pub struct Columns {
    layout:     ColumnLayout,
    columns:    Vec<usize>
}
impl Columns {
    fn new(layout: ColumnLayout) -> Self {
        let mut columns = Vec::new();
        let mut last = layout.para.last;
        while last > 0 {
            columns.push(last);
            last = layout.nodes_col[last].unwrap().prev;
        }
        Columns {
            layout: layout,
            columns: columns
        }
    }
    pub fn get_column(&self, n: usize) -> Column {
        let len = self.columns.len();
        assert!(n < len);
        let last = self.columns[len - 1 - n];
        Column::new(
            self.layout.nodes_col[last].unwrap().prev,
            last,
            &self.layout.para
        )
    }
    pub fn columns(&self) -> impl Iterator<Item=Column> {
        self.columns.iter().rev().map(move |&last| Column::new(
            self.layout.nodes_col[last].unwrap().prev,
            last,
            &self.layout.para
        ))
    }
    pub fn len(&self) -> usize {
        self.columns.len()
    }
    pub fn into_stream(self) -> StreamVec {
        StreamVec(self.layout.para.items)
    }
}

#[derive(Debug)]
pub struct Column<'a> {
    lines:      Vec<usize>, // points to the end of each line
    layout:     &'a ParagraphLayout,
    y:          Length
}
impl<'a> Column<'a> {
    fn new(first: usize, mut last: usize, layout: &'a ParagraphLayout) -> Self {
        let mut lines = Vec::new();
        while last > first {
            lines.push(last);
            last = layout.nodes[last].unwrap().prev;
        }
        
        Column {
            lines: lines,
            layout: layout,
            y: Length::zero()
        }
    }
}
impl<'a> Iterator for Column<'a> {
    type Item = (Length, Line<'a>);
    
    fn next(&mut self) -> Option<Self::Item> {
        self.lines.pop().map(|last| {
            let b = self.layout.nodes[last].unwrap();
            self.y += b.height;
            let y = self.y;

            if let Entry::Linebreak(_, skip, _) = self.layout.items[last-1] {
                self.y += skip;
            }
            
            (y, Line {
                layout:   self.layout,
                pos:      b.prev,
                branches: 0,
                measure:  FlexMeasure::zero(),
                line:     b,
                end:      last-1,
            })
        })
    }
}

#[derive(Debug)]
pub struct Line<'a> {
    layout:     &'a ParagraphLayout,
    pos:        usize,
    end:        usize,
    branches:   usize,
    measure:    FlexMeasure,
    line:       LineBreak,
}
impl<'a> Line<'a> {
    pub fn height(&self) -> Length {
        self.line.height
    }
}
impl<'a> Iterator for Line<'a> {
    type Item = (Length, Size, RenderItem, Tag);
    fn next(&mut self) -> Option<Self::Item> {
        while self.pos < self.end {
            let pos = self.pos;
            self.pos += 1;
            let is_first = self.pos != self.line.prev;

            match self.layout.items[pos] {
                Entry::Item(m, item, tag) => {
                    if is_first {
                        // the first item on the line gets moved to the left by m.left
                        self.measure -= m.left;
                    }

                    // take current location
                    let x = self.measure.at(self.line.factor) + self.line.indent;

                    // add the width of the item
                    self.measure += m.content;
                            
                    let size = Size::new(m.content.at(self.line.factor), m.height);
                    return Some((x, size, item, tag));
                },
                Entry::Space(s, _, _) => {
                    self.measure += s;
                },
                Entry::BranchEntry(len) => {
                    if self.line.path & (1<<self.branches) == 0 {
                        // not taken
                        self.pos += len;
                    }
                    self.branches += 1;
                },
                Entry::BranchExit(skip) => self.pos += skip,
                Entry::Linebreak(_, _, _) => unreachable!(),
                Entry::SetWidth(_, _) => {}
                Entry::Column => unreachable!()
            }
        }
        
        None
    }
}
