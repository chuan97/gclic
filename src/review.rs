//! Review comments on diffs, on a line or on the whole file: `gc` on a diff's
//! active line writes one, `gC` one on its file, the Review tab lists them, and `y` there copies them all as one review, to paste
//! to whoever made the changes. They last until cleared, or until gclic quits.

use crate::git::{self, DiffSource};
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Also read by the app, for the tab's label.
pub const TITLE: &str = "Review";

/// Which version of the file a line is in: the one the diff goes from, for a
/// removed line, or the one it goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Old,
    New,
}

/// A line of a diff: its side, its number in that version of the file, and
/// its text, which finds it again once the file has changed and the number
/// with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub side: Side,
    pub number: usize,
    pub text: String,
}

/// The changes a comment is about. The working tree's are one place, staged
/// or not, so a comment stays with its line when the file is staged.
#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    WorkingTree,
    Commit(String),
    Refs(String, String),
    Stash(String),
}

impl Place {
    /// Where a diff's changes are. None for a conflict, which isn't anyone's
    /// changes yet.
    pub fn of(source: &DiffSource) -> Option<Place> {
        Some(match source {
            DiffSource::Unstaged(_) | DiffSource::Staged(_) | DiffSource::Untracked(_) => Place::WorkingTree,
            DiffSource::Conflict(_) => return None,
            DiffSource::Commit { sha, .. } => Place::Commit(sha.clone()),
            DiffSource::Refs { a, b, .. } => Place::Refs(a.clone(), b.clone()),
            DiffSource::Stash { sha, .. } => Place::Stash(sha.clone()),
        })
    }

    /// How the copied review and the list name it: the commit with its
    /// subject, so the reader can tell which commit is meant.
    fn heading(&self) -> String {
        let short = |sha: &str| sha[..sha.len().min(7)].to_string();
        match self {
            Place::WorkingTree => "Uncommitted changes".to_string(),
            Place::Commit(sha) => match git::commit_label(sha) {
                Some(label) => match label.split_once("  ") {
                    Some((short, subject)) => format!("Commit {short} \"{subject}\""),
                    None => format!("Commit {label}"),
                },
                None => format!("Commit {}", short(sha)),
            },
            Place::Refs(a, b) => format!("Changes from {a} to {b}"),
            Place::Stash(sha) => format!("Stashed changes {}", short(sha)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    /// The diff it was written on, to show again from the Review tab.
    pub source: DiffSource,
    pub place: Place,
    /// The place's heading, worked out once, as a commit's needs git.
    heading: String,
    /// The line it's on. None for a comment on the whole file.
    pub anchor: Option<Anchor>,
    pub body: String,
}

/// A comment's line, as a key: its side and number, or None for the file.
pub type LineKey = Option<(Side, usize)>;

fn key_of(anchor: &Option<Anchor>) -> LineKey {
    anchor.as_ref().map(|anchor| (anchor.side, anchor.number))
}

impl Comment {
    pub fn new(source: DiffSource, anchor: Option<Anchor>, body: String) -> Option<Comment> {
        let place = Place::of(&source)?;
        let heading = place.heading();
        Some(Comment { source, place, heading, anchor, body })
    }

    pub fn path(&self) -> &str {
        self.source.path()
    }

    /// `path:line`, or for a removed line, which only the old version has,
    /// says so. Just the path for the whole file.
    pub fn location(&self) -> String {
        location(self.path(), &self.anchor)
    }

    /// Whether it's about this same line, or file, of this same diff.
    fn is_on(&self, place: &Place, path: &str, key: LineKey) -> bool {
        self.place == *place && self.path() == path && key_of(&self.anchor) == key
    }
}

/// Where a comment is, as the list, the box and the copied review say it.
pub fn location(path: &str, anchor: &Option<Anchor>) -> String {
    match anchor {
        None => path.to_string(),
        Some(Anchor { side: Side::New, number, .. }) => format!("{path}:{number}"),
        Some(Anchor { side: Side::Old, number, .. }) => format!("{path}, removed line {number}"),
    }
}

#[derive(Default)]
pub struct Review {
    comments: Vec<Comment>,
}

impl Review {
    pub fn len(&self) -> usize {
        self.comments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Comment> {
        self.comments.get(index)
    }

    /// The comment on this line, or on the file for None, if there is one.
    pub fn find(&self, source: &DiffSource, key: LineKey) -> Option<usize> {
        let place = Place::of(source)?;
        self.comments.iter().position(|comment| comment.is_on(&place, source.path(), key))
    }

    /// Adds a comment, or replaces the one already on its line. An empty one
    /// removes it.
    pub fn set(&mut self, comment: Comment) {
        let key = key_of(&comment.anchor);
        let existing = self.comments.iter().position(|c| c.is_on(&comment.place, comment.path(), key));
        match (existing, comment.body.trim().is_empty()) {
            (Some(index), true) => {
                self.comments.remove(index);
            }
            (Some(index), false) => self.comments[index] = comment,
            (None, true) => {}
            (None, false) => self.comments.push(comment),
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.comments.len() {
            self.comments.remove(index);
        }
    }

    pub fn clear(&mut self) {
        self.comments.clear();
    }

    /// The comments on this diff's file, to show in it.
    pub fn on_diff(&self, source: &DiffSource) -> Vec<(Option<Anchor>, String)> {
        let Some(place) = Place::of(source) else {
            return Vec::new();
        };
        self.comments
            .iter()
            .filter(|comment| comment.place == place && comment.path() == source.path())
            .map(|comment| (comment.anchor.clone(), comment.body.clone()))
            .collect()
    }

    /// The comments in the order they're listed and sent: by place, in the
    /// order each was first commented on, then by file, the file's own
    /// comments first, then down the file.
    pub fn ordered(&self) -> Vec<usize> {
        let mut places: Vec<&Place> = Vec::new();
        for comment in &self.comments {
            if !places.contains(&&comment.place) {
                places.push(&comment.place);
            }
        }
        let mut order: Vec<usize> = (0..self.comments.len()).collect();
        order.sort_by_key(|&i| {
            let comment = &self.comments[i];
            let place = places.iter().position(|place| **place == comment.place);
            (place, comment.path().to_string(), comment.anchor.as_ref().map(|anchor| anchor.number))
        });
        order
    }

    /// The comments as one review, for whoever made the changes: each under
    /// its place, with its file and line, the line itself, and the comment.
    pub fn text(&self) -> String {
        let count = match self.comments.len() {
            1 => "the comment".to_string(),
            _ => "each comment".to_string(),
        };
        let mut text = format!("Here's my review of your changes. Address {count}.\n");
        let mut heading = None;
        for index in self.ordered() {
            let comment = &self.comments[index];
            if heading != Some(&comment.heading) {
                heading = Some(&comment.heading);
                text.push_str(&format!("\n## {}\n", comment.heading));
            }
            text.push_str(&format!("\n{}\n", comment.location()));
            if let Some(anchor) = &comment.anchor {
                text.push_str(&format!("    {}\n", anchor.text.trim_end()));
            }
            for line in comment.body.trim_end().lines() {
                // A bare > for an empty line, without a trailing space.
                text.push_str(format!("> {line}").trim_end());
                text.push('\n');
            }
        }
        text
    }
}

/// The Review tab's list: the comments under their places' headings.
pub struct ReviewList {
    /// A position in `Review::ordered`.
    selected: usize,
    scroll: usize,
}

/// What a key in the list asks the app for.
pub enum ListAction {
    None,
    /// The selection moved: show its comment's line.
    Moved,
    /// Move into the diff.
    Enter,
    Edit(usize),
    Delete(usize),
    Copy,
    Clear,
}

impl ReviewList {
    pub fn new() -> ReviewList {
        ReviewList { selected: 0, scroll: 0 }
    }

    /// The selected comment, as an index into the review.
    pub fn selected(&self, review: &Review) -> Option<usize> {
        let order = review.ordered();
        order.get(self.selected.min(order.len().saturating_sub(1))).copied()
    }

    pub fn handle_key(&mut self, review: &Review, key: KeyEvent) -> ListAction {
        let len = review.len();
        if len == 0 {
            return ListAction::None;
        }
        self.selected = self.selected.min(len - 1);
        let before = self.selected;
        let selected = self.selected(review);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.selected = ui::next_index(self.selected, len),
            KeyCode::Char('k') | KeyCode::Up => self.selected = ui::previous_index(self.selected, len),
            KeyCode::Char('g') => self.selected = 0,
            KeyCode::Char('G') => self.selected = len - 1,
            KeyCode::Enter => return ListAction::Enter,
            KeyCode::Char('c') => return selected.map_or(ListAction::None, ListAction::Edit),
            KeyCode::Char('d') => return selected.map_or(ListAction::None, ListAction::Delete),
            KeyCode::Char('y') => return ListAction::Copy,
            KeyCode::Char('D') => return ListAction::Clear,
            _ => {}
        }
        if self.selected == before { ListAction::None } else { ListAction::Moved }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool, review: &Review) {
        let count = match review.len() {
            1 => "1 comment".to_string(),
            n => format!("{n} comments"),
        };
        let block = ui::panel(format!(" {TITLE} · {count} "), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let order = review.ordered();
        self.selected = self.selected.min(order.len().saturating_sub(1));
        let mut lines = Vec::new();
        let mut selected_line = 0;
        let mut heading = None;
        let mut headings = Vec::new();
        for (position, &index) in order.iter().enumerate() {
            let Some(comment) = review.get(index) else { continue };
            if heading != Some(&comment.heading) {
                heading = Some(&comment.heading);
                headings.push(lines.len());
                let style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
                lines.push(Line::styled(comment.heading.clone(), style));
            }
            if position == self.selected {
                selected_line = lines.len();
            }
            let first = comment.body.lines().next().unwrap_or_default().to_string();
            let mut line = Line::from(vec![
                Span::raw("  "),
                Span::styled(comment.location(), Style::default().fg(ui::DIM)),
                Span::raw("  "),
                Span::raw(first),
            ]);
            if focused && position == self.selected {
                line = line.style(Style::default().bg(ui::SELECTED_BG));
            }
            lines.push(line);
        }
        let height = inner.height as usize;
        self.scroll = ui::keep_visible(self.scroll, selected_line, height);
        // The heading above the first comment under it shows with it.
        if self.scroll == selected_line && headings.contains(&selected_line.wrapping_sub(1)) {
            self.scroll -= 1;
        }
        let shown = lines.into_iter().skip(self.scroll).collect();
        ui::render_lines(frame, inner, shown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(source: DiffSource, side: Side, number: usize, text: &str, body: &str) -> Comment {
        let anchor = Some(Anchor { side, number, text: text.to_string() });
        Comment {
            place: Place::of(&source).unwrap(),
            heading: match &source {
                DiffSource::Commit { .. } => "Commit a3f9c12 \"Name the sides\"".to_string(),
                _ => "Uncommitted changes".to_string(),
            },
            source,
            anchor,
            body: body.to_string(),
        }
    }

    fn unstaged(path: &str) -> DiffSource {
        DiffSource::Unstaged(path.to_string())
    }

    fn in_commit(path: &str) -> DiffSource {
        DiffSource::Commit { sha: "a3f9c12aaaa".to_string(), path: path.to_string() }
    }

    #[test]
    fn a_line_takes_one_comment_which_an_empty_one_removes() {
        let mut review = Review::default();
        review.set(comment(unstaged("a.rs"), Side::New, 3, "x", "first"));
        review.set(comment(unstaged("a.rs"), Side::New, 3, "x", "second"));
        assert_eq!(review.len(), 1);
        assert_eq!(review.get(0).unwrap().body, "second");
        // The same line staged is the same place.
        assert_eq!(review.find(&DiffSource::Staged("a.rs".to_string()), Some((Side::New, 3))), Some(0));
        review.set(comment(unstaged("a.rs"), Side::New, 3, "x", "  "));
        assert!(review.is_empty());
    }

    #[test]
    fn a_removed_line_and_an_added_one_with_the_same_number_are_apart() {
        let mut review = Review::default();
        review.set(comment(unstaged("a.rs"), Side::New, 3, "new", "on the new"));
        review.set(comment(unstaged("a.rs"), Side::Old, 3, "old", "on the old"));
        assert_eq!(review.len(), 2);
    }

    #[test]
    fn conflicts_take_no_comments() {
        let anchor = Some(Anchor { side: Side::New, number: 1, text: String::new() });
        assert!(Comment::new(DiffSource::Conflict("a.rs".to_string()), anchor, "x".to_string()).is_none());
    }

    #[test]
    fn the_review_groups_comments_by_place_then_file_then_line() {
        let mut review = Review::default();
        review.set(comment(unstaged("b.rs"), Side::New, 9, "    let b = 2;", "Why 2?"));
        review.set(comment(in_commit("a.rs"), Side::New, 4, "fn a() {}", "Rename this."));
        review.set(comment(unstaged("a.rs"), Side::Old, 12, "check();", "Keep this check.\n\nIt guards the empty case."));
        review.set(comment(unstaged("b.rs"), Side::New, 2, "use x;", "Unused."));
        let mut on_file = comment(unstaged("b.rs"), Side::New, 0, "", "Split this file.");
        on_file.anchor = None;
        review.set(on_file);
        assert_eq!(
            review.text(),
            "Here's my review of your changes. Address each comment.\n\
             \n## Uncommitted changes\n\
             \na.rs, removed line 12\n    check();\n> Keep this check.\n>\n> It guards the empty case.\n\
             \nb.rs\n> Split this file.\n\
             \nb.rs:2\n    use x;\n> Unused.\n\
             \nb.rs:9\n        let b = 2;\n> Why 2?\n\
             \n## Commit a3f9c12 \"Name the sides\"\n\
             \na.rs:4\n    fn a() {}\n> Rename this.\n"
        );
    }

    #[test]
    fn the_list_moves_through_comments_in_their_order() {
        let mut review = Review::default();
        review.set(comment(unstaged("b.rs"), Side::New, 9, "b", "second"));
        review.set(comment(unstaged("a.rs"), Side::New, 1, "a", "first"));
        let mut list = ReviewList::new();
        assert_eq!(list.selected(&review), Some(1));
        assert!(matches!(list.handle_key(&review, KeyEvent::from(KeyCode::Char('j'))), ListAction::Moved));
        assert_eq!(list.selected(&review), Some(0));
        assert!(matches!(list.handle_key(&review, KeyEvent::from(KeyCode::Char('d'))), ListAction::Delete(0)));
    }
}
