//! The diff pane: its rows laid out side by side or unified, the active
//! line, the minimap, and review comments under their lines.

use super::cache::*;
use super::parse::*;
use super::patch::ChangeLines;
use crate::git::{self, DiffSource, Repo};
use crate::review::{Anchor, Side};
use crate::theme::BUILT_IN;
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::{HashMap, HashSet};

// The background, text and tints behind the code come from `theme`: the
// editor's, inside one, or else VS Code Dark Modern's, which gclic comes with.
const FILLER_BG: Color = Color::Indexed(235);
// A conflict's two sides, as VS Code tints them: green for the current
// change, ours, and blue for the incoming one, theirs. Ours is yours in a
// merge, but upstream in a rebase, so the colours keep to the sides and the
// title says what they are.
const CURRENT_BG: Color = Color::Rgb(26, 52, 46);
const CURRENT_WORD_BG: Color = Color::Rgb(40, 98, 84);
const INCOMING_BG: Color = Color::Rgb(26, 42, 64);
const INCOMING_WORD_BG: Color = Color::Rgb(42, 74, 116);
/// Columns moved by one sideways scroll.
const SIDE_STEP: usize = 4;
/// The line number in front of each side's text, as drawn by `side_line`.
const GUTTER_WIDTH: usize = 5;
/// A review comment's lines, under the line it's on, and the mark beside that
/// line's number.
const NOTE_BG: Color = Color::Rgb(46, 42, 22);
const NOTE_MARK: Color = Color::Yellow;

/// One line on screen. Side by side, each row is one. Unified, a change's
/// rows split into their removed lines, then their added ones.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Item {
    /// A whole row: a heading, context, or side by side, a change.
    Row(usize),
    /// A row's left side, on its own line.
    Old(usize),
    /// A row's right side, on its own line.
    New(usize),
}

impl Item {
    fn row(self) -> usize {
        match self {
            Item::Row(row) | Item::Old(row) | Item::New(row) => row,
        }
    }
}

/// Whether a row has a changed line on either side.
fn row_is_changed(row: &Row) -> bool {
    let changed = |cell: &Option<Cell>| cell.as_ref().is_some_and(|cell| cell.changed);
    matches!(row, Row::Lines { left, right } if changed(left) || changed(right))
}

/// The lines to show for these rows, side by side or unified. A whole file
/// is one hunk, whose heading would only repeat the file's counts, so it
/// leaves hunk headings out.
fn layout(rows: &[Row], split: bool, whole: bool) -> Vec<Item> {
    let shown = |i: &usize| !(whole && matches!(rows[*i], Row::Hunk { .. }));
    if split {
        return (0..rows.len()).filter(shown).map(Item::Row).collect();
    }
    let mut items = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        if !row_is_changed(&rows[i]) {
            if shown(&i) {
                items.push(Item::Row(i));
            }
            i += 1;
            continue;
        }
        let end = (i..rows.len())
            .find(|&j| !row_is_changed(&rows[j]))
            .unwrap_or(rows.len());
        let has = |j: &usize, left: bool| match &rows[*j] {
            Row::Lines { left: l, right: r } => {
                if left {
                    l.is_some()
                } else {
                    r.is_some()
                }
            }
            Row::Heading(_) | Row::Hunk { .. } => false,
        };
        items.extend((i..end).filter(|j| has(j, true)).map(Item::Old));
        items.extend((i..end).filter(|j| has(j, false)).map(Item::New));
        i = end;
    }
    items
}

/// Side by side when each side gets at least this many columns of text, and
/// unified otherwise, unless the user chose one with `s`.
const SPLIT_MIN_TEXT: usize = 80;
/// Unified, the line numbers of both versions come before the text.
const UNIFIED_GUTTER_WIDTH: usize = 2 * GUTTER_WIDTH;
/// The minimap's columns, one for each side, and the gap before them.
const MINIMAP_WIDTH: u16 = 3;
const MINIMAP_TRACK: Color = Color::Indexed(235);
const MINIMAP_VIEW: Color = Color::Indexed(240);
/// Changes are brighter where they're on screen, so the view still shows
/// over them.
const MINIMAP_REMOVED: [Color; 2] = [BUILT_IN.removed_word, Color::Rgb(205, 80, 88)];
const MINIMAP_ADDED: [Color; 2] = [BUILT_IN.added_word, Color::Rgb(88, 184, 112)];
const MINIMAP_CURRENT: [Color; 2] = [CURRENT_WORD_BG, Color::Rgb(72, 180, 146)];
const MINIMAP_INCOMING: [Color; 2] = [INCOMING_WORD_BG, Color::Rgb(90, 146, 220)];

/// The file's lines with a review comment, by side and number.
type CommentedLines = HashSet<(Side, usize)>;

/// The first of `total` lines to show in `height` rows so that `position` is
/// in the middle, without going past either end.
fn scroll_to(position: usize, total: usize, height: usize) -> usize {
    position
        .saturating_sub(height / 2)
        .min(total.saturating_sub(height))
}

/// The file's lines a line on screen shows, by side, as comments are put:
/// a removed line on the old side, any other on the new one. Side by side, a
/// row shows both lines of a change.
fn item_lines(rows: &[Row], item: Item) -> Vec<(Side, usize)> {
    let Row::Lines { left, right } = &rows[item.row()] else {
        return Vec::new();
    };
    let old = left
        .as_ref()
        .filter(|cell| cell.changed)
        .map(|cell| (Side::Old, cell.number));
    let new = right.as_ref().map(|cell| (Side::New, cell.number));
    match item {
        Item::Old(_) => old.into_iter().collect(),
        Item::New(_) => new.into_iter().collect(),
        Item::Row(_) => new.into_iter().chain(old).collect(),
    }
}

/// The number a comment's line has now: its own, if the line there is still
/// the same, or else that of the nearest line on its side with its text, as
/// the file may have changed above it. None if no such line is in the diff.
fn locate(rows: &[Row], anchor: &Anchor) -> Option<usize> {
    let cells = rows.iter().filter_map(|row| match (row, anchor.side) {
        (Row::Lines { left, .. }, Side::Old) => left.as_ref().filter(|cell| cell.changed),
        (Row::Lines { right, .. }, Side::New) => right.as_ref(),
        (Row::Heading(_) | Row::Hunk { .. }, _) => None,
    });
    cells
        .filter(|cell| cell.text == anchor.text)
        .map(|cell| cell.number)
        .min_by_key(|number| number.abs_diff(anchor.number))
}

/// Where each row sits in the measured file, as a line index from 0. A row
/// with no line on that side, such as a removed line or a heading, sits
/// where the next line does, as that's where it would be inserted.
fn file_positions(rows: &[Row]) -> Vec<usize> {
    let new_side = measures_new_side(rows);
    let mut positions = vec![0; rows.len()];
    let mut next = rows
        .iter()
        .filter_map(|row| side_number(row, new_side))
        .max()
        .unwrap_or(0);
    for (index, row) in rows.iter().enumerate().rev() {
        if let Some(number) = side_number(row, new_side) {
            next = number.saturating_sub(1);
        }
        positions[index] = next;
    }
    positions
}

fn side_number(row: &Row, new_side: bool) -> Option<usize> {
    match row {
        Row::Lines { left, right } => if new_side { right } else { left }
            .as_ref()
            .map(|cell| cell.number),
        Row::Heading(_) | Row::Hunk { .. } => None,
    }
}

/// The length of the file the minimap measures. Until it's known, the last
/// line the diff reaches stands in.
fn file_length(rows: &[Row], file_lines: usize) -> usize {
    let new_side = measures_new_side(rows);
    file_lines.max(
        rows.iter()
            .filter_map(|row| side_number(row, new_side))
            .max()
            .unwrap_or(0),
    )
}

/// One of the minimap's rows: whether it covers removed lines, added lines,
/// and lines on screen.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct MapCell {
    removed: bool,
    added: bool,
    in_view: bool,
}

/// The whole file squeezed into `height` rows, each covering an equal share
/// of its lines, with the changes and the part on screen marked. `visible`
/// are the rows of the diff on screen. Empty when the file fits anyway.
fn minimap(rows: &[Row], file_lines: usize, height: usize, visible: &[usize]) -> Vec<MapCell> {
    let positions = file_positions(rows);
    let total = file_length(rows, file_lines);
    if height == 0 || total <= height {
        return Vec::new();
    }
    let mut cells = vec![MapCell::default(); height];
    let cell_of = |row: usize| (positions[row] * height / total).min(height - 1);
    for (index, row) in rows.iter().enumerate() {
        if let Row::Lines { left, right } = row {
            let cell = &mut cells[cell_of(index)];
            cell.removed |= left.as_ref().is_some_and(|cell| cell.changed);
            cell.added |= right.as_ref().is_some_and(|cell| cell.changed);
        }
    }
    for &row in visible {
        cells[cell_of(row)].in_view = true;
    }
    cells
}

pub struct DiffPane {
    pub source: DiffSource,
    /// The whole file, or just the changes, as its text was loaded.
    pub whole: bool,
    /// The diff as git printed it, to tell whether a reload changed anything.
    text: String,
    rows: Vec<Row>,
    /// The lines shown, made from `rows` for the layout in use.
    items: Vec<Item>,
    /// Side by side, or unified.
    split: bool,
    /// The length of the file the minimap measures, once known.
    file_lines: usize,
    /// The line under the cursor, an index into `items`. The view keeps it in
    /// the middle, except near the ends, where it moves on without scrolling.
    active: usize,
    height: usize,
    /// How many characters each side's text is scrolled to the left.
    shift: usize,
    /// Room for text on each side, past the line numbers.
    text_width: usize,
    /// Whether the rows have their syntax colours yet. The worker thread
    /// makes them, and `apply_colors` takes them.
    colored: bool,
    /// Whether the user has stepped to a change with n or N, which makes the
    /// active line worth showing even while another pane has focus.
    navigated: bool,
    /// Whether this is a conflict met while rebasing, whose sides are named
    /// the other way round from a merge's.
    rebasing: bool,
    /// The review's comments on this file, each with the line it's on, to
    /// draw under their lines, or none for the file's own, drawn at the top.
    comments: Vec<(Option<Anchor>, String)>,
}

/// Whether this is a conflict met while rebasing.
fn rebasing(repo: &Repo, source: &DiffSource) -> bool {
    matches!(source, DiffSource::Conflict(_))
        && git::operation_in_progress(repo) == Some("rebasing")
}

/// The length, in characters, of the longest line on either side.
fn longest_line(rows: &[Row]) -> usize {
    let cells = rows.iter().flat_map(|row| match row {
        Row::Lines { left, right } => [left.as_ref(), right.as_ref()],
        Row::Heading(_) | Row::Hunk { .. } => [None, None],
    });
    cells
        .flatten()
        .map(|cell| cell.text.chars().count())
        .max()
        .unwrap_or(0)
}

impl DiffPane {
    /// Loads without syntax colours, which can take a moment for a big file.
    /// They come from the cache or the worker thread, via `apply_colors`.
    pub fn load(repo: &Repo, view: DiffView) -> Result<DiffPane> {
        let text = git::diff(repo, &view.source, view.whole)?;
        let rows = rows_for(&view.source, &text);
        let rebasing = rebasing(repo, &view.source);
        Ok(DiffPane::new(
            view,
            text,
            Colored {
                rows,
                file_lines: 0,
            },
            false,
            rebasing,
        ))
    }

    /// A diff already coloured, as the cache keeps them, without asking git
    /// for it.
    pub fn colored(repo: &Repo, view: DiffView, text: String, colored: Colored) -> DiffPane {
        let rebasing = rebasing(repo, &view.source);
        DiffPane::new(view, text, colored, true, rebasing)
    }

    fn new(
        view: DiffView,
        text: String,
        colored: Colored,
        is_colored: bool,
        rebasing: bool,
    ) -> DiffPane {
        let DiffView { source, whole } = view;
        let Colored { rows, file_lines } = colored;
        let mut pane = DiffPane {
            rebasing,
            source,
            whole,
            text,
            items: layout(&rows, true, whole),
            rows,
            split: true,
            file_lines,
            active: 0,
            height: 0,
            shift: 0,
            text_width: 0,
            colored: is_colored,
            navigated: false,
            comments: Vec::new(),
        };
        // The whole file opens on its first change, which may be far down,
        // as only the changes open at their first hunk. It's selected, as
        // after n, so the next n goes on to the second.
        if whole && let Some(first) = (0..pane.items.len()).find(|&i| pane.starts_change(i)) {
            pane.active = first;
            pane.navigated = true;
        }
        pane
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn needs_colors(&self) -> bool {
        !self.colored
    }

    /// Whether it showed side by side when last drawn.
    pub fn is_split(&self) -> bool {
        self.split
    }

    /// Takes coloured rows, if they were made from the diff on screen. Ones
    /// for an older version of it are dropped.
    pub fn apply_colors(&mut self, text: &str, colored: Colored) {
        if self.text == text {
            self.set_rows(colored.rows);
            self.file_lines = colored.file_lines;
            self.colored = true;
        }
    }

    /// Fetches the diff again, for files that changed while it was on screen.
    pub fn reload(&mut self, repo: &Repo) -> Result<()> {
        self.set_text(git::diff(repo, &self.source, self.whole)?);
        Ok(())
    }

    /// Fetches it again as the whole file, or just the changes.
    pub fn reload_as(&mut self, repo: &Repo, whole: bool) -> Result<()> {
        let text = git::diff(repo, &self.source, whole)?;
        self.whole = whole;
        self.set_text(text);
        Ok(())
    }

    /// The diff as the cache and the worker know it.
    pub fn view(&self) -> DiffView {
        DiffView {
            source: self.source.clone(),
            whole: self.whole,
        }
    }

    /// Takes a newer version of the diff, which needs colouring again. Nothing
    /// happens if it hasn't changed, as is usual on a refresh.
    pub fn set_text(&mut self, text: String) {
        if text == self.text {
            return;
        }
        self.set_rows(rows_for(&self.source, &text));
        self.colored = false;
        self.text = text;
    }

    fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.items = layout(&self.rows, self.split, self.whole);
        self.active = self.active.min(self.items.len().saturating_sub(1));
    }

    /// Switches between side by side and unified, staying on the same row.
    fn set_split(&mut self, split: bool) {
        if split == self.split {
            return;
        }
        let row = self.items.get(self.active).map_or(0, |item| item.row());
        self.split = split;
        self.items = layout(&self.rows, split, self.whole);
        self.active = self
            .items
            .iter()
            .position(|item| item.row() == row)
            .unwrap_or(0);
    }

    /// The first line on screen without comments: the one that centres the
    /// active line, without going past either end. Drawing does the same,
    /// counting the comments' lines.
    #[cfg(test)]
    fn scroll(&self) -> usize {
        scroll_to(self.active, self.items.len(), self.height)
    }

    /// The review's comments on this file, which show under their lines.
    pub fn set_comments(&mut self, comments: Vec<(Option<Anchor>, String)>) {
        self.comments = comments;
    }

    /// Whether the active line shows: while focused, or once n or N has
    /// moved it, from any pane.
    pub fn shows_active_line(&self, focused: bool) -> bool {
        focused || self.navigated
    }

    /// The line under the cursor, to comment on: the new version's, or a
    /// removed line's, which only the old version has. None on a heading.
    pub fn active_anchor(&self) -> Option<Anchor> {
        let item = *self.items.get(self.active)?;
        let Row::Lines { left, right } = &self.rows[item.row()] else {
            return None;
        };
        let (side, cell) = match item {
            Item::Old(_) => (Side::Old, left.as_ref()?),
            Item::New(_) => (Side::New, right.as_ref()?),
            Item::Row(_) => match (right, left) {
                (Some(cell), _) => (Side::New, cell),
                (None, Some(cell)) => (Side::Old, cell),
                (None, None) => return None,
            },
        };
        Some(Anchor {
            side,
            number: cell.number,
            text: cell.text.clone(),
        })
    }

    /// The comment on the active line, if it has one, as it was written:
    /// editing it replaces it, even if its line has moved since.
    pub fn comment_at_active(&self) -> Option<Anchor> {
        let item = *self.items.get(self.active)?;
        let lines = item_lines(&self.rows, item);
        let here = |anchor: &Anchor| {
            locate(&self.rows, anchor).is_some_and(|number| lines.contains(&(anchor.side, number)))
        };
        self.comments
            .iter()
            .filter_map(|(anchor, _)| anchor.as_ref())
            .find(|anchor| here(anchor))
            .cloned()
    }

    /// Moves the active line to the one a comment is on, wherever it has
    /// moved to since, or to the top for a comment on the file. Shown even
    /// without focus, as after n.
    pub fn go_to(&mut self, anchor: Option<&Anchor>) {
        let Some(anchor) = anchor else {
            self.active = 0;
            return;
        };
        let Some(number) = locate(&self.rows, anchor) else {
            return;
        };
        if let Some(index) = (0..self.items.len())
            .find(|&i| item_lines(&self.rows, self.items[i]).contains(&(anchor.side, number)))
        {
            self.active = index;
            self.navigated = true;
        }
    }

    /// Where the comments go: the lines of each under the line on screen it's
    /// on, and the file's lines that have one, for their marks. Comments
    /// whose line is no longer in the diff aren't shown.
    /// Each comment's lines are wrapped to `width`.
    fn placed_comments(&self, width: usize) -> (HashMap<usize, Vec<String>>, CommentedLines) {
        let mut notes: HashMap<usize, Vec<String>> = HashMap::new();
        let mut marked = HashSet::new();
        if self.comments.is_empty() {
            return (notes, marked);
        }
        let mut item_of = HashMap::new();
        for (index, item) in self.items.iter().enumerate() {
            for line in item_lines(&self.rows, *item) {
                item_of.entry(line).or_insert(index);
            }
        }
        for (anchor, body) in &self.comments {
            // The file's own go at the top, under the first heading.
            let Some(anchor) = anchor else {
                if !self.items.is_empty() {
                    let lines = body
                        .trim_end()
                        .lines()
                        .flat_map(|line| ui::wrap(line, width));
                    notes.entry(0).or_default().extend(lines);
                }
                continue;
            };
            let Some(number) = locate(&self.rows, anchor) else {
                continue;
            };
            let Some(&index) = item_of.get(&(anchor.side, number)) else {
                continue;
            };
            marked.insert((anchor.side, number));
            let lines = body
                .trim_end()
                .lines()
                .flat_map(|line| ui::wrap(line, width));
            notes.entry(index).or_default().extend(lines);
        }
        (notes, marked)
    }

    /// The line number in the current file at the active row. Rows without
    /// one, such as hunk headings and removed lines, take the next line's, or
    /// the previous one's at the very end.
    pub fn active_line(&self) -> Option<usize> {
        let new_line = |row: &Row| match row {
            Row::Lines {
                right: Some(cell), ..
            } => Some(cell.number),
            _ => None,
        };
        let active = match self.items.get(self.active) {
            Some(Item::New(row)) => return new_line(&self.rows[*row]),
            Some(item) => item.row(),
            None => 0,
        };
        let below = self.rows.iter().skip(active).find_map(new_line);
        below.or_else(|| self.rows.iter().take(active).rev().find_map(new_line))
    }

    /// Back to the start of the lines, for when the diff loses focus, as h and l
    /// stop scrolling it then.
    pub fn reset_shift(&mut self) {
        self.shift = 0;
    }

    /// The furthest sideways scroll that still shows the end of the longest line.
    fn max_shift(&self) -> usize {
        longest_line(&self.rows).saturating_sub(self.text_width)
    }

    fn title(&self) -> String {
        let title = match &self.source {
            DiffSource::Unstaged(path) => format!(" {path} "),
            DiffSource::Untracked(path) => format!(" {path} (untracked) "),
            DiffSource::Conflict(path) => {
                format!(" {path} (conflict: {}) ", conflict_sides(self.rebasing))
            }
            DiffSource::Staged(path) => format!(" {path} (staged) "),
            DiffSource::Commit { sha, path } => format!(" {path} @ {} ", git::short(sha)),
            DiffSource::Refs { a, b, path } => format!(" {path}: {a} → {b} "),
            DiffSource::Stash {
                path,
                untracked: false,
                ..
            } => format!(" {path} (stashed) "),
            DiffSource::Stash { path, .. } => format!(" {path} (stashed, untracked) "),
        };
        // A new file or a conflict shows whole anyway.
        let whole = self.whole
            && !matches!(
                self.source,
                DiffSource::Untracked(_) | DiffSource::Conflict(_)
            );
        if whole {
            format!("{title}· whole file ")
        } else {
            title
        }
    }

    /// How many changes there are, and which one the active line is in, if any.
    fn change_position(&self) -> (Option<usize>, usize) {
        let starts: Vec<usize> = (0..self.items.len())
            .filter(|&i| self.starts_change(i))
            .collect();
        let current = self
            .is_changed(self.active)
            .then(|| starts.iter().filter(|&&start| start <= self.active).count());
        (current, starts.len())
    }

    /// "change 3 of 12" while on a change, or how many there are otherwise.
    fn change_counter(&self) -> Option<String> {
        let noun = if matches!(self.source, DiffSource::Conflict(_)) {
            "conflict"
        } else {
            "change"
        };
        match self.change_position() {
            (_, 0) => None,
            (Some(current), total) => Some(format!(" {noun} {current} of {total} ")),
            (None, 1) => Some(format!(" 1 {noun} ")),
            (None, total) => Some(format!(" {total} {noun}s ")),
        }
    }

    /// The lines of the change the active line is in, if it's in one.
    pub fn active_change(&self) -> Option<ChangeLines> {
        if !self.is_changed(self.active) {
            return None;
        }
        let start = (0..=self.active)
            .rev()
            .take_while(|&i| self.is_changed(i))
            .last()?;
        let end = (self.active..self.items.len())
            .take_while(|&i| self.is_changed(i))
            .last()?;
        let mut change = ChangeLines::default();
        let changed = |cell: &Option<Cell>| {
            cell.as_ref()
                .filter(|cell| cell.changed)
                .map(|cell| cell.number)
        };
        for item in &self.items[start..=end] {
            let Row::Lines { left, right } = &self.rows[item.row()] else {
                continue;
            };
            match item {
                Item::Row(_) => {
                    change.old.extend(changed(left));
                    change.new.extend(changed(right));
                }
                Item::Old(_) => change.old.extend(changed(left)),
                Item::New(_) => change.new.extend(changed(right)),
            }
        }
        Some(change)
    }

    /// How many changes the diff has.
    pub fn change_count(&self) -> usize {
        self.change_position().1
    }

    /// Onto the start of a change, after the diff changed under the active
    /// line: the one it's in, or else the next, or else the last before it.
    pub fn go_to_nearest_change(&mut self) {
        let start_back = (0..=self.active.min(self.items.len().saturating_sub(1)))
            .rev()
            .find(|&i| self.starts_change(i));
        let target = if self.is_changed(self.active) {
            start_back
        } else {
            (self.active..self.items.len())
                .find(|&i| self.starts_change(i))
                .or(start_back)
        };
        if let Some(target) = target {
            self.active = target;
            self.navigated = true;
        }
    }

    /// Moves the active line, stopping at the ends.
    pub fn handle_key(&mut self, key: KeyEvent) {
        let last = self.items.len().saturating_sub(1);
        let half_page = (self.height / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('d') if ctrl => self.active = (self.active + half_page).min(last),
            KeyCode::Char('u') if ctrl => self.active = self.active.saturating_sub(half_page),
            KeyCode::Char('j') | KeyCode::Down => self.active = (self.active + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.active = self.active.saturating_sub(1),
            KeyCode::Char('g') => self.active = 0,
            KeyCode::Char('G') => self.active = last,
            KeyCode::Char('h') | KeyCode::Left => self.shift = self.shift.saturating_sub(SIDE_STEP),
            KeyCode::Char('l') | KeyCode::Right => {
                self.shift = (self.shift + SIDE_STEP).min(self.max_shift());
            }
            // Between changes rather than hunks, so it still works when a whole
            // file is one hunk. From inside a change, N goes to its start.
            KeyCode::Char('n') => {
                self.navigated = true;
                let next = (self.active + 1..self.items.len()).find(|&i| self.starts_change(i));
                self.active = next.unwrap_or(self.active);
            }
            KeyCode::Char('N') => {
                self.navigated = true;
                let previous = (0..self.active).rev().find(|&i| self.starts_change(i));
                self.active = previous.unwrap_or(self.active);
            }
            _ => {}
        }
    }

    /// Whether a line on screen is part of a change.
    fn is_changed(&self, index: usize) -> bool {
        match self.items.get(index) {
            Some(Item::Row(row)) => row_is_changed(&self.rows[*row]),
            Some(Item::Old(_) | Item::New(_)) => true,
            None => false,
        }
    }

    /// Whether a change starts at this line: a changed line at the top, or
    /// after one that isn't changed, such as context or a hunk's heading.
    fn starts_change(&self, index: usize) -> bool {
        self.is_changed(index) && (index == 0 || !self.is_changed(index - 1))
    }

    /// Draws the diff. `split` is the user's choice of layout, if they made
    /// one; otherwise it depends on the room there is.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool, split: Option<bool>) {
        let mut block = ui::panel(self.title(), focused);
        if let Some(counter) = self.change_counter() {
            block = block
                .title_bottom(Line::styled(counter, Style::default().fg(ui::DIM)).right_aligned());
        }
        let inner = block.inner(area);
        frame.render_widget(block, area);
        // The code's own background, inside the border. What's drawn on it
        // keeps it, unless it sets a background of its own.
        let colors = crate::theme::diff_colors();
        frame.render_widget(
            ratatui::widgets::Block::new()
                .style(Style::default().bg(colors.background).fg(colors.text)),
            inner,
        );
        self.height = inner.height as usize;

        if self.rows.is_empty() {
            let note = Line::styled("No changes", Style::default().fg(ui::DIM));
            frame.render_widget(note, inner);
            return;
        }

        let has_map = file_length(&self.rows, self.file_lines) > self.height
            && inner.width > MINIMAP_WIDTH * 4;
        let content = if has_map {
            Rect {
                width: inner.width - MINIMAP_WIDTH,
                ..inner
            }
        } else {
            inner
        };
        let column_width = content.width.saturating_sub(1) / 2;
        let auto = (column_width as usize).saturating_sub(GUTTER_WIDTH) >= SPLIT_MIN_TEXT;
        self.set_split(split.unwrap_or(auto));

        // What's on screen, line by line: the diff's lines, each followed by
        // the lines of the comments on it, if any.
        let indent = if self.split {
            GUTTER_WIDTH
        } else {
            UNIFIED_GUTTER_WIDTH
        };
        let (notes, marked) =
            self.placed_comments((content.width as usize).saturating_sub(indent + 1));
        let mut screen: Vec<(usize, Option<&str>)> = Vec::with_capacity(self.items.len());
        for index in 0..self.items.len() {
            screen.push((index, None));
            for note in notes.get(&index).into_iter().flatten() {
                screen.push((index, Some(note.as_str())));
            }
        }
        let at = screen
            .iter()
            .position(|&(index, note)| index == self.active && note.is_none())
            .unwrap_or(0);
        let scroll = scroll_to(at, screen.len(), self.height);
        let shown = &screen[scroll.min(screen.len())..(scroll + self.height).min(screen.len())];
        if has_map {
            let visible: Vec<usize> = shown
                .iter()
                .map(|&(index, _)| self.items[index].row())
                .collect();
            let map = minimap(&self.rows, self.file_lines, self.height, &visible);
            let map_area = Rect {
                x: inner.right() - MINIMAP_WIDTH + 1,
                width: MINIMAP_WIDTH - 1,
                ..inner
            };
            self.render_minimap(frame, map_area, &map);
        }
        self.text_width = if self.split {
            // Both sides get the same room, as the right one only gains a spare column.
            (column_width as usize).saturating_sub(GUTTER_WIDTH)
        } else {
            (content.width as usize).saturating_sub(UNIFIED_GUTTER_WIDTH)
        };
        // The pane may have widened since the last scroll.
        self.shift = self.shift.min(self.max_shift());

        let conflict = matches!(self.source, DiffSource::Conflict(_));
        let (old_colors, new_colors) = if conflict {
            (
                (CURRENT_BG, CURRENT_WORD_BG),
                (INCOMING_BG, INCOMING_WORD_BG),
            )
        } else {
            (
                (colors.removed, colors.removed_word),
                (colors.added, colors.added_word),
            )
        };
        let has_mark = |side: Side, cell: Option<&Cell>| {
            cell.is_some_and(|cell| {
                (side == Side::New || cell.changed) && marked.contains(&(side, cell.number))
            })
        };
        for (i, &(index, note)) in shown.iter().enumerate() {
            let full = Rect {
                y: content.y + i as u16,
                height: 1,
                ..content
            };
            let item = self.items[index];
            if let Some(note) = note {
                // Under the line it's on, past where line numbers go.
                let line = Line::from(vec![
                    Span::raw(" ".repeat(indent.saturating_sub(2))),
                    Span::styled("▌ ", Style::default().fg(NOTE_MARK)),
                    Span::raw(note.to_string()),
                ]);
                frame.render_widget(line.style(Style::default().bg(NOTE_BG)), full);
                continue;
            }
            // Like a list's selection, the active line only shows while focused,
            // or once n or N has moved it, as then it's where the user is.
            let active = (focused || self.navigated) && index == self.active;
            let row = &self.rows[item.row()];
            let heading = match row {
                Row::Heading(text) => Some(Line::styled(
                    text.as_str(),
                    Style::default().fg(ui::accent()),
                )),
                Row::Hunk {
                    line,
                    added,
                    removed,
                    context,
                } => Some(hunk_heading(*line, *added, *removed, context)),
                Row::Lines { .. } => None,
            };
            if let Some(line) = heading {
                let line = if active {
                    line.style(Style::default().bg(ui::GREY_BG))
                } else {
                    line
                };
                frame.render_widget(line, full);
                continue;
            }
            let Row::Lines { left, right } = row else {
                continue;
            };
            let (left, right) = (left.as_ref(), right.as_ref());
            match item {
                Item::Row(_) if self.split => {
                    let left_area = Rect {
                        width: column_width,
                        ..full
                    };
                    let separator = Rect {
                        x: full.x + column_width,
                        width: 1,
                        ..full
                    };
                    let right_area = Rect {
                        x: full.x + column_width + 1,
                        width: content.width.saturating_sub(column_width + 1),
                        ..full
                    };
                    let (left_bg, left_word) = old_colors;
                    let (right_bg, right_word) = new_colors;
                    let left_line = side_line(
                        left,
                        self.shift,
                        (active, has_mark(Side::Old, left)),
                        left_bg,
                        left_word,
                    );
                    let right_line = side_line(
                        right,
                        self.shift,
                        (active, has_mark(Side::New, right)),
                        right_bg,
                        right_word,
                    );
                    frame.render_widget(left_line, left_area);
                    frame.render_widget(Line::styled("│", Style::default().fg(ui::DIM)), separator);
                    frame.render_widget(right_line, right_area);
                }
                // Context, with both line numbers.
                Item::Row(_) => {
                    let numbers = (left.map(|cell| cell.number), right.map(|cell| cell.number));
                    let mark = has_mark(Side::New, right) || has_mark(Side::Old, left);
                    if let Some(cell) = right.or(left) {
                        let line =
                            unified_line(numbers, cell, self.shift, (active, mark), new_colors);
                        frame.render_widget(line, full);
                    }
                }
                Item::Old(_) => {
                    if let Some(cell) = left {
                        let flags = (active, has_mark(Side::Old, left));
                        let line = unified_line(
                            (Some(cell.number), None),
                            cell,
                            self.shift,
                            flags,
                            old_colors,
                        );
                        frame.render_widget(line, full);
                    }
                }
                Item::New(_) => {
                    if let Some(cell) = right {
                        let flags = (active, has_mark(Side::New, right));
                        let line = unified_line(
                            (None, Some(cell.number)),
                            cell,
                            self.shift,
                            flags,
                            new_colors,
                        );
                        frame.render_widget(line, full);
                    }
                }
            }
        }
    }

    /// Two columns, the old side's changes and the new side's, over a track
    /// that's lighter where the lines on screen are.
    fn render_minimap(&self, frame: &mut Frame, area: Rect, map: &[MapCell]) {
        let conflict = matches!(self.source, DiffSource::Conflict(_));
        let (old_color, new_color) = if conflict {
            (MINIMAP_CURRENT, MINIMAP_INCOMING)
        } else {
            (MINIMAP_REMOVED, MINIMAP_ADDED)
        };
        for (y, cell) in map.iter().enumerate() {
            let track = if cell.in_view {
                MINIMAP_VIEW
            } else {
                MINIMAP_TRACK
            };
            let side = |marked: bool, colors: [Color; 2]| {
                Span::styled(
                    " ",
                    Style::default().bg(if marked {
                        colors[cell.in_view as usize]
                    } else {
                        track
                    }),
                )
            };
            let line = Line::from(vec![
                side(cell.removed, old_color),
                side(cell.added, new_color),
            ]);
            frame.render_widget(
                line,
                Rect {
                    y: area.y + y as u16,
                    height: 1,
                    ..area
                },
            );
        }
    }
}

fn gutter_style(active: bool) -> Style {
    if active {
        Style::default()
            .fg(Color::White)
            .bg(ui::GREY_BG)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ui::DIM)
    }
}

/// What a conflict's two sides are, left and right. In a merge, ours is the
/// branch checked out, yours, and theirs the one coming in. A rebase replays
/// your commits onto upstream, so there ours is upstream, with the commits
/// replayed so far, and theirs is the commit being replayed, your own.
fn conflict_sides(rebasing: bool) -> &'static str {
    if rebasing {
        "upstream ↔ your commit"
    } else {
        "yours ↔ theirs"
    }
}

/// One side of a row, with its text scrolled `shift` characters to the left.
/// The line number stays put, and stands out on the active row. Only the number
/// does, so that the colours of a change stay readable.
fn side_line(
    cell: Option<&Cell>,
    shift: usize,
    (active, marked): (bool, bool),
    changed_bg: Color,
    word_bg: Color,
) -> Line<'_> {
    let Some(cell) = cell else {
        return Line::default().style(Style::default().bg(FILLER_BG));
    };
    finish_line(
        gutter(format!("{:>4}", cell.number), active, marked),
        cell,
        shift,
        changed_bg,
        word_bg,
    )
}

/// The line numbers, standing out on the active line, then a mark if the
/// line has a review comment, or a space.
fn gutter(numbers: String, active: bool, marked: bool) -> Vec<Span<'static>> {
    let mark = if marked {
        Span::styled("●", Style::default().fg(NOTE_MARK))
    } else {
        Span::styled(" ", gutter_style(active))
    };
    vec![Span::styled(numbers, gutter_style(active)), mark]
}

/// A line of a unified diff: the old and new line numbers, either of which
/// may be blank, then the text.
fn unified_line(
    (old, new): (Option<usize>, Option<usize>),
    cell: &Cell,
    shift: usize,
    (active, marked): (bool, bool),
    (changed_bg, word_bg): (Color, Color),
) -> Line<'_> {
    let number = |n: Option<usize>| n.map_or("    ".to_string(), |n| format!("{n:>4}"));
    let numbers = format!("{} {}", number(old), number(new));
    finish_line(
        gutter(numbers, active, marked),
        cell,
        shift,
        changed_bg,
        word_bg,
    )
}

/// The gutter, then the visible text, cut wherever a syntax colour or a
/// changed word starts or ends, so each piece has one text colour and one
/// background.
fn finish_line<'a>(
    gutter: Vec<Span<'a>>,
    cell: &'a Cell,
    shift: usize,
    changed_bg: Color,
    word_bg: Color,
) -> Line<'a> {
    let mut spans = gutter;
    let text = &cell.text;
    let start = text
        .char_indices()
        .nth(shift)
        .map_or(text.len(), |(i, _)| i);
    let mut cuts = vec![start, text.len()];
    let edges = cell
        .marks
        .iter()
        .chain(cell.colors.iter().map(|(range, _)| range));
    cuts.extend(
        edges
            .flat_map(|range| [range.start, range.end])
            .filter(|&cut| cut > start),
    );
    cuts.sort_unstable();
    cuts.dedup();
    for piece in cuts.windows(2) {
        let (from, to) = (piece[0], piece[1].min(text.len()));
        if from >= to {
            continue;
        }
        let mut style = Style::default();
        if let Some((_, color)) = cell.colors.iter().find(|(range, _)| range.contains(&from)) {
            style = style.fg(*color);
        }
        if cell.marks.iter().any(|mark| mark.contains(&from)) {
            style = style.bg(word_bg);
        }
        spans.push(Span::styled(&text[from..to], style));
    }
    let line = Line::from(spans);
    if cell.changed {
        line.style(Style::default().bg(changed_bg))
    } else {
        line
    }
}

/// A hunk's heading: `line 6  +43 −2`, then what it's inside of, dimmed.
fn hunk_heading(line: usize, added: usize, removed: usize, context: &str) -> Line<'_> {
    let mut spans = vec![Span::styled(
        format!("line {line}"),
        Style::default().fg(ui::accent()),
    )];
    let counts = ui::line_count_spans(git::Lines::Counted { added, removed });
    if !counts.is_empty() {
        spans.push(Span::raw("  "));
        spans.extend(counts);
    }
    if !context.is_empty() {
        spans.push(Span::styled(
            format!("  {context}"),
            Style::default().fg(ui::DIM),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
// Marks are lists of ranges, so a list holding one range is meant.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::diff::testing::cell;

    fn pane_with_rows(count: usize, height: usize) -> DiffPane {
        let rows = vec![Row::Heading("x".to_string()); count];
        let mut pane = pane_of(rows);
        pane.height = height;
        pane
    }

    fn pane_of(rows: Vec<Row>) -> DiffPane {
        let source = DiffSource::Unstaged("f".to_string());
        DiffPane::new(
            DiffView {
                source,
                whole: false,
            },
            String::new(),
            Colored {
                rows,
                file_lines: 0,
            },
            true,
            false,
        )
    }

    fn text_of(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn scrolling_sideways_keeps_the_line_number_and_the_marks() {
        let cell = Cell {
            number: 7,
            text: "héllo wörld".to_string(),
            changed: true,
            marks: vec![7..13],
            colors: Vec::new(),
        };
        let line = side_line(
            Some(&cell),
            3,
            (false, false),
            BUILT_IN.added,
            BUILT_IN.added_word,
        );
        assert_eq!(text_of(&line), "   7 lo wörld");
        let marked = line
            .spans
            .iter()
            .find(|span| span.style.bg == Some(BUILT_IN.added_word));
        assert_eq!(marked.unwrap().content, "wörld");
    }

    #[test]
    fn syntax_colours_and_changed_words_combine() {
        let (red, blue) = (Color::Rgb(255, 0, 0), Color::Rgb(0, 0, 255));
        let cell = Cell {
            number: 1,
            text: "let x = 1;".to_string(),
            changed: true,
            marks: vec![8..9],
            colors: vec![(0..3, red), (3..10, blue)],
        };
        let line = side_line(
            Some(&cell),
            0,
            (false, false),
            BUILT_IN.added,
            BUILT_IN.added_word,
        );
        // After the line number and the space for a comment's mark.
        let pieces: Vec<_> = line.spans[2..]
            .iter()
            .map(|span| (span.content.as_ref(), span.style.fg, span.style.bg))
            .collect();
        assert_eq!(
            pieces,
            [
                ("let", Some(red), None),
                (" x = ", Some(blue), None),
                ("1", Some(blue), Some(BUILT_IN.added_word)),
                (";", Some(blue), None),
            ]
        );
    }

    #[test]
    fn scrolling_past_a_short_line_leaves_only_its_number() {
        let cell = Cell {
            number: 1,
            text: "ab".to_string(),
            changed: false,
            marks: Vec::new(),
            colors: Vec::new(),
        };
        assert_eq!(
            text_of(&side_line(
                Some(&cell),
                10,
                (false, false),
                BUILT_IN.added,
                BUILT_IN.added_word
            )),
            "   1 "
        );
    }

    #[test]
    fn scrolling_sideways_stops_once_the_longest_line_ends() {
        let mut pane = pane_with_rows(1, 10);
        pane.set_rows(vec![Row::Lines {
            left: cell(1, &"x".repeat(10), false),
            right: None,
        }]);
        pane.text_width = 4;
        for _ in 0..5 {
            pane.handle_key(KeyEvent::from(KeyCode::Char('l')));
        }
        assert_eq!(pane.shift, 6);
        pane.handle_key(KeyEvent::from(KeyCode::Char('h')));
        assert_eq!(pane.shift, 2);
    }

    #[test]
    fn a_rebase_s_conflict_names_its_sides_the_other_way_round() {
        assert_eq!(conflict_sides(false), "yours ↔ theirs");
        assert_eq!(conflict_sides(true), "upstream ↔ your commit");
    }

    #[test]
    fn colours_for_another_version_of_the_diff_are_dropped() {
        let mut pane = pane_with_rows(1, 10);
        pane.text = "new".to_string();
        pane.colored = false;
        pane.apply_colors("old", Colored::default());
        assert!(pane.needs_colors());
        pane.apply_colors("new", Colored::default());
        assert!(!pane.needs_colors());
    }

    #[test]
    fn n_and_shift_n_jump_between_changes_even_in_one_hunk() {
        // One hunk holding two separate changes, as a whole-file diff does.
        let mut pane = pane_with_rows(0, 10);
        pane.set_rows(parse_diff(
            "@@ -1,7 +1,7 @@\n a\n-b\n+B\n c\n d\n-e\n-f\n+E\n g",
        ));
        let press = |pane: &mut DiffPane, ch| pane.handle_key(KeyEvent::from(KeyCode::Char(ch)));
        press(&mut pane, 'n');
        assert_eq!(pane.active, 2); // b → B
        press(&mut pane, 'n');
        assert_eq!(pane.active, 5); // e → E
        press(&mut pane, 'n');
        assert_eq!(pane.active, 5); // no more changes
        press(&mut pane, 'j'); // inside the change, onto f
        press(&mut pane, 'N');
        assert_eq!(pane.active, 5); // back to its start
        press(&mut pane, 'N');
        assert_eq!(pane.active, 2);
    }

    fn press(pane: &mut DiffPane, ch: char) {
        pane.handle_key(KeyEvent::from(KeyCode::Char(ch)));
    }

    #[test]
    fn unified_shows_a_changes_removed_lines_then_its_added_ones() {
        let rows = parse_diff("@@ -1,3 +1,3 @@\n a\n-b\n-c\n+B\n+C\n d");
        // Side by side: a, b|B, c|C, d after the heading.
        assert_eq!(layout(&rows, true, false).len(), 5);
        assert_eq!(
            layout(&rows, false, false),
            [
                Item::Row(0),
                Item::Row(1),
                Item::Old(2),
                Item::Old(3),
                Item::New(2),
                Item::New(3),
                Item::Row(4)
            ]
        );
    }

    #[test]
    fn unified_skips_the_empty_side_of_a_row() {
        let rows = parse_diff("@@ -1,2 +1,3 @@\n a\n+new\n b");
        assert_eq!(
            layout(&rows, false, false),
            [Item::Row(0), Item::Row(1), Item::New(2), Item::Row(3)]
        );
    }

    #[test]
    fn a_whole_file_opens_at_its_first_change() {
        let text = "@@ -1,7 +1,7 @@\n a\n b\n c\n-d\n+D\n e\n-f\n+F\n g";
        let open = |whole| {
            DiffPane::new(
                DiffView {
                    source: DiffSource::Unstaged("f".to_string()),
                    whole,
                },
                String::new(),
                Colored {
                    rows: parse_diff(text),
                    file_lines: 0,
                },
                true,
                false,
            )
        };
        let mut pane = open(true);
        pane.height = 20;
        assert_eq!(pane.active, 3); // d → D
        assert_eq!(pane.active_line(), Some(4));
        // Selected, as after n, from any pane.
        assert!(pane.shows_active_line(false));
        assert_eq!(pane.change_counter().as_deref(), Some(" change 1 of 2 "));
        // So n goes on to the second.
        press(&mut pane, 'n');
        assert_eq!(pane.active, 5);
        // Just the changes open at the top, the first hunk's heading, with
        // nothing selected.
        let changes = open(false);
        assert_eq!(changes.active, 0);
        assert!(!changes.shows_active_line(false));
    }

    #[test]
    fn a_whole_file_has_no_hunk_heading() {
        let rows = parse_diff("@@ -1,2 +1,3 @@\n a\n+new\n b");
        assert_eq!(
            layout(&rows, true, true),
            [Item::Row(1), Item::Row(2), Item::Row(3)]
        );
        assert_eq!(
            layout(&rows, false, true),
            [Item::Row(1), Item::New(2), Item::Row(3)]
        );
    }

    #[test]
    fn switching_layout_stays_on_the_same_row() {
        let mut pane = pane_of(parse_diff("@@ -1,3 +1,3 @@\n a\n-b\n-c\n+B\n+C\n d"));
        pane.height = 20;
        pane.active = 4; // d
        pane.set_split(false);
        assert_eq!(pane.items[pane.active], Item::Row(4));
        pane.active = 4; // B, unified
        pane.set_split(true);
        assert_eq!(pane.active, 2); // the row b|B
    }

    #[test]
    fn n_jumps_over_a_whole_unified_change() {
        let mut pane = pane_of(parse_diff("@@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n-d\n+D\n e"));
        pane.height = 20;
        pane.set_split(false);
        press(&mut pane, 'n');
        assert_eq!(pane.items[pane.active], Item::Old(2));
        press(&mut pane, 'n');
        assert_eq!(pane.items[pane.active], Item::Old(4));
        assert_eq!(pane.change_counter().as_deref(), Some(" change 2 of 2 "));
    }

    fn anchor(side: Side, number: usize, text: &str) -> Anchor {
        Anchor {
            side,
            number,
            text: text.to_string(),
        }
    }

    #[test]
    fn a_comment_goes_on_a_removed_line_or_the_new_version_s() {
        let mut pane = pane_of(parse_diff("@@ -1,2 +1,2 @@\n-x\n+y\n w"));
        pane.set_split(false);
        // Unified: x removed, y added, then w.
        let at = |pane: &mut DiffPane, active| {
            pane.active = active;
            pane.active_anchor()
        };
        assert_eq!(at(&mut pane, 0), None, "a hunk's heading");
        assert_eq!(at(&mut pane, 1), Some(anchor(Side::Old, 1, "x")));
        assert_eq!(at(&mut pane, 2), Some(anchor(Side::New, 1, "y")));
        assert_eq!(at(&mut pane, 3), Some(anchor(Side::New, 2, "w")));
        // Side by side, a change's row is the new line's.
        pane.set_split(true);
        assert_eq!(at(&mut pane, 1), Some(anchor(Side::New, 1, "y")));
    }

    #[test]
    fn a_comment_finds_its_line_after_lines_above_it_moved() {
        let rows = parse_diff("@@ -1,3 +1,4 @@\n a\n+new\n b\n c");
        // Written on c when it was line 3; it's line 4 now.
        assert_eq!(locate(&rows, &anchor(Side::New, 3, "c")), Some(4));
        assert_eq!(locate(&rows, &anchor(Side::New, 4, "c")), Some(4));
        assert_eq!(locate(&rows, &anchor(Side::New, 3, "gone")), None);
        // Of two lines with its text, the nearer one.
        let rows = parse_diff("@@ -1,3 +1,3 @@\n }\n x\n }");
        assert_eq!(locate(&rows, &anchor(Side::New, 3, "}")), Some(3));
    }

    #[test]
    fn going_to_a_comment_makes_its_line_the_active_one() {
        let mut pane = pane_of(parse_diff("@@ -1,3 +1,4 @@\n a\n+new\n b\n c"));
        pane.go_to(Some(&anchor(Side::New, 3, "c")));
        assert_eq!(pane.active_line(), Some(4));
        assert!(pane.navigated);
    }

    #[test]
    fn a_comment_shows_under_its_line_with_a_mark_beside_it() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut pane = pane_of(parse_diff("@@ -1,2 +1,2 @@\n a\n-b\n+c"));
        pane.set_comments(vec![(
            Some(anchor(Side::New, 2, "c")),
            "Why c?\nb was fine.".to_string(),
        )]);
        assert_eq!(pane.comment_at_active(), None);
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal
            .draw(|frame| pane.render(frame, frame.area(), true, Some(false)))
            .unwrap();
        let screen: Vec<String> = (0..8)
            .map(|y| {
                (0..60)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect();
        let line = |text: &str| screen.iter().position(|row| row.contains(text)).unwrap();
        let commented = &screen[line("   2●")];
        assert!(commented.contains('c'), "{commented}");
        assert_eq!(line("Why c?"), line("   2●") + 1);
        assert_eq!(line("b was fine."), line("   2●") + 2);
        pane.active = 3;
        assert_eq!(pane.comment_at_active(), Some(anchor(Side::New, 2, "c")));
    }

    #[test]
    fn the_active_line_on_an_added_line_is_its_own() {
        let mut pane = pane_of(parse_diff("@@ -1,2 +1,3 @@\n-x\n+y\n+z\n w"));
        pane.set_split(false);
        pane.active = 3; // z, after x, y
        assert_eq!(pane.items[pane.active], Item::New(2));
        assert_eq!(pane.active_line(), Some(2));
    }

    #[test]
    fn the_counter_says_which_change_or_how_many() {
        let mut pane = pane_of(parse_diff("@@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n-d\n+D\n e"));
        assert_eq!(pane.change_counter().as_deref(), Some(" 2 changes "));
        press(&mut pane, 'n');
        assert_eq!(pane.change_counter().as_deref(), Some(" change 1 of 2 "));
        press(&mut pane, 'j');
        assert_eq!(pane.change_counter().as_deref(), Some(" 2 changes "));
        assert_eq!(
            pane_of(parse_diff("@@ -1 +1 @@\n a")).change_counter(),
            None
        );
    }

    #[test]
    fn removed_lines_sit_where_the_next_line_is() {
        let rows = parse_diff("@@ -1,3 +1,2 @@\n a\n-gone\n b\n c");
        // heading, a, gone, b, c: at the new file's lines 1, 1, 2, 2, 3.
        assert_eq!(file_positions(&rows), [0, 0, 1, 1, 2]);
    }

    #[test]
    fn a_deleted_file_is_measured_by_its_old_lines() {
        let rows = parse_diff("@@ -1,2 +0,0 @@\n-a\n-b");
        assert_eq!(file_positions(&rows), [0, 0, 1]);
    }

    #[test]
    fn the_minimap_marks_changes_and_the_view_across_the_whole_file() {
        // Changes at lines 10 and 90 of a 100-line file, in 10 rows.
        let rows = parse_diff("@@ -10 +10 @@\n-x\n+X\n@@ -90,0 +91 @@\n+new");
        let map = minimap(&rows, 100, 10, &[0, 1]);
        let marked: Vec<_> = map.iter().map(|c| (c.removed, c.added)).collect();
        assert_eq!(marked[0], (true, true)); // line 10 is in the first tenth
        assert_eq!(marked[9], (false, true)); // line 91, added only
        assert!(marked[1..9].iter().all(|m| *m == (false, false)));
        let in_view: Vec<_> = map.iter().map(|c| c.in_view).collect();
        assert_eq!(in_view.iter().filter(|v| **v).count(), 1);
        assert!(in_view[0]);
    }

    #[test]
    fn no_minimap_for_a_file_that_fits() {
        let rows = parse_diff("@@ -1 +1 @@\n-x\n+X");
        assert!(minimap(&rows, 5, 10, &[0]).is_empty());
        // Until the length is known, the diff's last line stands in.
        assert_eq!(file_length(&rows, 0), 1);
    }

    #[test]
    fn the_active_row_stops_at_both_ends() {
        let mut pane = pane_with_rows(5, 2);
        pane.handle_key(KeyEvent::from(KeyCode::Char('k')));
        assert_eq!(pane.active, 0);
        pane.handle_key(KeyEvent::from(KeyCode::Char('G')));
        pane.handle_key(KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(pane.active, 4);
    }

    #[test]
    fn the_view_centres_the_active_row_except_near_the_ends() {
        let mut pane = pane_with_rows(20, 6);
        pane.active = 10;
        assert_eq!(pane.scroll(), 7);
        pane.active = 1;
        assert_eq!(pane.scroll(), 0);
        pane.active = 19;
        assert_eq!(pane.scroll(), 14);
    }

    #[test]
    fn a_diff_that_fits_on_screen_doesnt_scroll() {
        let mut pane = pane_with_rows(2, 10);
        pane.handle_key(KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(pane.scroll(), 0);
    }

    #[test]
    fn the_active_line_is_the_next_one_in_the_current_file() {
        let mut pane = pane_with_rows(0, 10);
        pane.set_rows(parse_diff("@@ -4,2 +4,1 @@\n-gone\n same\n-also gone"));
        // The heading and a removed line both lead to `same`, line 4 now.
        assert_eq!(pane.active_line(), Some(4));
        pane.active = 1;
        assert_eq!(pane.active_line(), Some(4));
        // At the end, with nothing below, it takes the line above.
        pane.active = 3;
        assert_eq!(pane.active_line(), Some(4));
    }
}
