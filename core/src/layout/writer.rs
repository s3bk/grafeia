use crate::layout::*;

#[derive(Debug)]
pub struct Writer {
    state:      Glue,
    stream:     StreamVec,
    branch_stack: BranchStack,
    /// The last item was a margin number: the glue in front of the next item
    /// is the word space of the text the number hangs beside, and it must not
    /// appear just because the number is there.
    after_margin: bool,
}

#[derive(Debug)]
enum BranchState {
    // initially at Pre
    Pre,
    // then we get called before A gets pushed
    A { entry_pos: usize },
    // and again after A was pushed and before B is pushed
    B { exit_pos: usize },
    // and again after B was pushed
    Post
}
impl BranchState {
    fn step(&mut self, stream: &mut StreamVec) {
        let pos = stream.len();
        *self = match *self {
            BranchState::Pre => {
                stream.push(Entry::BranchEntry(0));

                BranchState::A { entry_pos: pos }
            },
            BranchState::A { entry_pos } => {
                // figure out the length of a
                stream.push(Entry::BranchEntry(0));
                
                let len_a = pos - entry_pos;
                stream.set(entry_pos, Entry::BranchEntry(len_a));

                BranchState::B { exit_pos: pos }
            }
            BranchState::B { exit_pos } => {
                let len_b = pos - exit_pos - 1;
                stream.set(exit_pos, Entry::BranchExit(len_b));

                BranchState::Post
            },
            BranchState::Post => panic!()
        };
    }
}

#[derive(Debug)]
struct Branch {
    size: usize,
    state: BranchState,
}

#[derive(Default, Debug)]
struct BranchStack {
    items: Vec<Branch>
}
impl BranchStack {
    fn init(&mut self, n: usize) {
        self.items.clear();
        if n > 1 {
            self.items.push(Branch {
                size: n,
                state: BranchState::Pre
            });
        }
    }
    fn step(&mut self, stream: &mut StreamVec) {
        while self.items.len() > 0 {
            let last = self.items.last_mut().unwrap();
            if let BranchState::B { .. } = last.state {
                last.state.step(stream);
                self.items.pop();
                continue;
            }

            match last {
                &mut Branch { size: 2, ref mut state } => {
                    state.step(stream);
                    return;
                },
                &mut Branch { size, ref mut state } if size > 2 => {
                    state.step(stream);
                    let half = size / 2;
                    let branch_size = match *state {
                        BranchState::A { .. } => half,
                        BranchState::B { .. } => size - half,
                        _ => unreachable!()
                    };
                    if branch_size == 1 {
                        // good to go
                        return;
                    }
                    self.items.push(Branch {
                        size: branch_size,
                        state: BranchState::Pre
                    });
                }
                b => panic!("{:?}", b)
            }
        }
    }
    fn finish(&mut self, stream: &mut StreamVec) {
        self.step(stream);
        assert_eq!(self.items.len(), 0);
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer {
            state:  Glue::None,
            stream: StreamVec::new(),
            branch_stack: BranchStack::default(),
            after_margin: false,
        }
    }
    pub fn with_stream(stream: StreamVec) -> Writer {
        Writer {
            state:  Glue::None,
            stream,
            branch_stack: BranchStack::default(),
            after_margin: false,
        }
    }
    pub fn finish(mut self) -> StreamVec {
        self.write_glue(Glue::any());
        self.stream
    }
    pub fn clear(&mut self) {
        self.state = Glue::None;
        self.stream.0.clear();
        self.after_margin = false;
    }
    
    #[inline(always)]
    fn write_glue(&mut self, left: Glue) {
        match self.state | left {
            Glue::Column => self.stream.push(Entry::Column),
            Glue::Newline { fill, vfill, height, column_break } => {
                self.stream.push(Entry::Linebreak(fill, vfill, height, column_break));
            },
            Glue::Space { measure, line_break, column_break }
             => self.stream.push(Entry::Space(measure, line_break, column_break)),
            Glue::None => ()
        }
    }
    
    fn push(&mut self, left: Glue, right: Glue, entry: Entry) {
        // A margin item hangs beside the line and takes no room. The glue in
        // front of the next item is the paragraph's own word space, which must
        // not appear just because a number sits before the first word — the
        // line has to break exactly where it would have without the number.
        // (`Empty` items are the sequence markers; they do not clear the flag.)
        let swallow = self.after_margin;
        if !swallow {
            self.write_glue(left);
        }
        self.after_margin = match &entry {
            Entry::Item(_, RenderItem::Margin(..), _) => true,
            Entry::Item(_, RenderItem::Empty, _) => self.after_margin,
            // Any other item, glue or line break ends the run.
            _ => false,
        };
        self.stream.push(entry);
        self.state = right;
    }

    pub fn item(&mut self, left: Glue, right: Glue, measure: ItemMeasure, item: RenderItem, tag: Tag) {
        self.push(left, right, Entry::Item(measure, item, tag));
    }
    pub fn space(&mut self, left: Glue, right: Glue, measure: FlexMeasure, line_break: Option<f32>, column_break: Option<f32>) {
        self.push(left, right, Entry::Space(measure, line_break, column_break));
    }
    pub fn set_width(&mut self, indent: Length, width: Length) {
        self.stream.push(Entry::SetWidth(indent, width));
    }

    #[inline(always)]
    pub fn promote(&mut self, glue: Glue) {
        self.state |= glue;
    }

    // much faster variant, but expects exactly count branches (calls to f)
    pub fn branch2(&mut self, count: usize, f: impl FnOnce(&mut Gen2)) {
        self.branch_stack.init(count);
        let state = self.state;
        let after_margin = self.after_margin;
        f(&mut Gen2 {
            writer: self,
            count,
            at: 0,
            state,
            after_margin
        });
        self.branch_stack.finish(&mut self.stream);
    }
}

#[derive(Debug)]
pub struct Gen2<'a> {
    writer: &'a mut Writer,
    count: usize,
    at: usize,
    /// The glue state at the branch entry. Every alternative starts from it:
    /// without this, the glue at the end of one alternative would be merged
    /// into the first item of the next, and a word alternative that ends in a
    /// space would give the hyphenated alternative a space it never asked for.
    state: Glue,
    /// Whether the last item was a margin number when the branch was entered:
    /// every alternative starts from there, like the glue state.
    after_margin: bool,
}
impl<'a> Gen2<'a> {
    pub fn add(&mut self, f: impl FnOnce(&mut Writer)) {
        assert!(self.at < self.count);
        self.writer.branch_stack.step(&mut self.writer.stream);
        self.writer.state = self.state;
        self.writer.after_margin = self.after_margin;
        f(self.writer);
        self.at += 1;
    }
}
