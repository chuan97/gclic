//! The side-by-side diff pane, used by every feature: diffs parsed into
//! rows, the coloured ones kept for going back to, and the pane drawing them.

mod cache;
mod pane;
mod parse;

pub use cache::{ColorCache, Colored, DiffView};
pub use pane::DiffPane;
pub use parse::{highlight_rows, rows_for};
#[cfg(test)]
pub use parse::Row;

/// Rows for the tests of each part.
#[cfg(test)]
mod testing {
    use super::parse::{Cell, Row};
    use super::Colored;

    pub fn cell(number: usize, text: &str, changed: bool) -> Option<Cell> {
        Some(Cell { number, text: text.to_string(), changed, marks: Vec::new(), colors: Vec::new() })
    }

    pub fn colored(rows: Vec<Row>) -> Colored {
        Colored { rows, file_lines: 0 }
    }
}
