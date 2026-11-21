use std::ops::{BitOr, BitOrAssign};
use std::fmt;
use super::{FlexMeasure, Length};

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Glue {
    None,
    Space {
        line_break:     Option<f32>,
        column_break:   Option<f32>,
        measure:        FlexMeasure
    },
    Newline {
        column_break:   Option<f32>,
        fill:           bool,
        height:         Length
    },
    Column
}
impl Glue {
    pub fn hfill(height: Length) -> Glue {
        Glue::Newline {
            fill: true,
            column_break: Some(0.),
            height
        }
    } 

    /// A fill break with a column-break penalty of its own: a negative value
    /// keeps an element with what follows, a large positive one demands the
    /// break (a page break the layout may not reason its way out of).
    pub fn hfill_with(height: Length, column_break: f32) -> Glue {
        Glue::Newline {
            fill: true,
            column_break: Some(column_break),
            height
        }
    }

    /// Demand a page break at the next break point. Expressed as a penalty
    /// rather than a separate entry, so it merges into the break that is
    /// already there: no empty line and no blank page.
    pub fn page_break() -> Glue {
        Glue::hfill_with(Length::zero(), Glue::PAGE_BREAK)
    }

    /// Big enough to outweigh every other penalty in the block algorithm.
    pub const PAGE_BREAK: f32 = 500_000.0;
}

fn merge_break(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        _ => None
    }
}

impl BitOr for Glue {
    type Output = Glue;
    
    fn bitor(self, rhs: Glue) -> Glue {
        use self::Glue::*;
        
        match (self, rhs) {
            // Glue::None wins over anything else
            (None, _) | (_, None) => None,
            
            (Space { line_break: Option::None, .. }, Newline { .. }) |
            (Newline { .. }, Space { line_break: Option::None, .. }) => {
                panic!("Newline and NonBreaking requested");
            },
            
            // NonBreaking wins over Breaking
            (Space { line_break: lb_a, column_break: cb_a, measure: a }, Space { line_break: lb_b, column_break: cb_b, measure: b })
                => Space { line_break: merge_break(lb_a, lb_b), column_break: merge_break(cb_a, cb_b), measure: a.max(b) },
            
            // Column wins
            (Column, _) | (_, Column) => Column,

            // Newline wins over Space
            (Newline { fill, height, column_break }, Space { .. }) | (Space { .. }, Newline { fill, height, column_break })
                => Newline { fill, height, column_break },
            
            (Newline { fill: a, height: h_a, column_break: cb_a }, Newline { fill: b, height: h_b, column_break: cb_b })
                => Newline { fill: a | b, height: h_a.max(h_b), column_break: merge_break(cb_a, cb_b) }
        }
    }
}
impl BitOrAssign for Glue {
    fn bitor_assign(&mut self, rhs: Glue) {
        *self = *self | rhs;
    }
}

impl Glue {
    pub fn nbspace(measure: FlexMeasure) -> Glue {
        Glue::Space { line_break: None, column_break: None, measure }
    }
    pub fn any() -> Glue {
        Glue::Space { line_break: Some(0.0), column_break: Some(0.0), measure: FlexMeasure::zero() }
    }
}

impl fmt::Display for Glue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Glue::None => Ok(()),
            Glue::Space { line_break: Some(_), .. } => write!(f, "␣"),
            Glue::Space { line_break: None, .. } => write!(f, "~"),
            Glue::Newline { .. } => write!(f, "␤"),
            Glue::Column => write!(f, "|")
        }
    }
}

