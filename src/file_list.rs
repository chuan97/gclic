//! The compare view's list of changed files, shown as a tree of folders, plus
//! the file row shared with the other panels' flat lists.

use crate::app::Action;
use crate::git::{DiffSource, FileEntry};
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::{BTreeMap, HashSet};

pub struct FileList {
    a: String,
    b: String,
    /// In the order the tree shows them, so moving through them goes down.
    files: Vec<FileEntry>,
    /// Every row of the tree, folded or not.
    rows: Vec<TreeRow>,
    /// The folders folded away, by path. All start open.
    collapsed: HashSet<String>,
    /// The row under the cursor, among the visible ones.
    cursor: usize,
    scroll: usize,
}

impl FileList {
    pub fn new(a: String, b: String, files: Vec<FileEntry>) -> FileList {
        let mut list = FileList {
            a,
            b,
            files: Vec::new(),
            rows: Vec::new(),
            collapsed: HashSet::new(),
            cursor: 0,
            scroll: 0,
        };
        list.set_files(files);
        list
    }

    /// Takes a newer list, keeping folded folders folded and the cursor on the
    /// same folder or file, if it's still there. A list that was empty, as a
    /// comparison's is until it loads, puts the cursor on the first file, so
    /// enter opens a diff at once.
    pub fn set_files(&mut self, files: Vec<FileEntry>) {
        let here = self.visible().get(self.cursor).map(|row| self.row_key(row));
        (self.files, self.rows) = tree(files);
        let visible = self.visible();
        self.cursor = match here {
            Some(key) => {
                let found = visible.iter().position(|row| self.row_key(row) == key);
                found.unwrap_or(self.cursor).min(visible.len().saturating_sub(1))
            }
            None => visible.iter().position(|row| matches!(row, TreeRow::File { .. })).unwrap_or(0),
        };
    }

    /// Going the other way, an added file is a deleted one and the reverse.
    pub fn swap_refs(&mut self) {
        std::mem::swap(&mut self.a, &mut self.b);
        for file in &mut self.files {
            file.status = match file.status {
                'A' => 'D',
                'D' => 'A',
                other => other,
            };
        }
    }

    #[cfg(test)]
    pub fn statuses(&self) -> Vec<char> {
        self.files.iter().map(|file| file.status).collect()
    }

    /// The rows shown: all of them, except inside folded folders.
    fn visible(&self) -> Vec<&TreeRow> {
        let mut visible = Vec::new();
        // Rows deeper than this are inside a folded folder.
        let mut folded_at: Option<usize> = None;
        for row in &self.rows {
            if folded_at.is_some_and(|depth| row.depth() > depth) {
                continue;
            }
            folded_at = match row {
                TreeRow::Folder { path, depth, .. } if self.collapsed.contains(path) => Some(*depth),
                _ => None,
            };
            visible.push(row);
        }
        visible
    }

    /// What a row stands for, to find it again in a newer list.
    fn row_key(&self, row: &TreeRow) -> String {
        match row {
            TreeRow::Folder { path, .. } => format!("{path}/"),
            TreeRow::File { index, .. } => self.files[*index].path.clone(),
        }
    }

    /// The file under the cursor. None on a folder.
    fn selected_file(&self) -> Option<usize> {
        match self.visible().get(self.cursor) {
            Some(TreeRow::File { index, .. }) => Some(*index),
            _ => None,
        }
    }

    pub fn selected_source(&self) -> Option<DiffSource> {
        self.source_at(self.selected_file()?)
    }

    /// The diffs one key away, most likely first, for prefetching.
    pub fn sources_around(&self) -> Vec<DiffSource> {
        let visible = self.visible();
        let positions = ui::one_key_away(self.cursor, visible.len());
        positions
            .into_iter()
            .filter_map(|position| match visible[position] {
                TreeRow::File { index, .. } => self.source_at(*index),
                TreeRow::Folder { .. } => None,
            })
            .collect()
    }

    fn source_at(&self, index: usize) -> Option<DiffSource> {
        let path = self.files.get(index)?.path.clone();
        Some(DiffSource::Refs { a: self.a.clone(), b: self.b.clone(), path })
    }

    /// Moves over files and folders. `h` folds a folder, or goes up to the
    /// folder a row is in; `l` unfolds a folder, or goes into an open one.
    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        let visible_len = self.visible().len();
        let before = self.cursor;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.cursor = ui::next_index(self.cursor, visible_len),
            KeyCode::Char('k') | KeyCode::Up => self.cursor = ui::previous_index(self.cursor, visible_len),
            KeyCode::Char('g') => self.cursor = 0,
            KeyCode::Char('G') => self.cursor = visible_len.saturating_sub(1),
            KeyCode::Char('h') | KeyCode::Left => self.fold_or_go_up(),
            KeyCode::Char('l') | KeyCode::Right => self.unfold_or_go_in(),
            KeyCode::Enter => {
                if let Some(path) = self.folder_under_cursor() {
                    self.toggle(path);
                    return Action::None;
                }
                return match self.selected_source() {
                    Some(source) => Action::OpenDiff(source),
                    None => Action::None,
                };
            }
            _ => {}
        }
        if self.cursor == before {
            return Action::None;
        }
        // On a folder the diff stays on the last file.
        match self.selected_source() {
            Some(source) => Action::PreviewDiff(source),
            None => Action::None,
        }
    }

    fn folder_under_cursor(&self) -> Option<String> {
        match self.visible().get(self.cursor) {
            Some(TreeRow::Folder { path, .. }) => Some(path.clone()),
            _ => None,
        }
    }

    fn toggle(&mut self, path: String) {
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
    }

    fn fold_or_go_up(&mut self) {
        if let Some(path) = self.folder_under_cursor()
            && !self.collapsed.contains(&path)
        {
            self.collapsed.insert(path);
            return;
        }
        // Up to the nearest folder above that holds this row.
        let visible = self.visible();
        let Some(depth) = visible.get(self.cursor).map(|row| row.depth()) else {
            return;
        };
        let parent = (0..self.cursor).rev().find(|&i| visible[i].depth() < depth);
        if let Some(parent) = parent {
            self.cursor = parent;
        }
    }

    fn unfold_or_go_in(&mut self) {
        let Some(path) = self.folder_under_cursor() else {
            return;
        };
        if !self.collapsed.remove(&path) {
            // Already open: into its first row.
            self.cursor = ui::next_index(self.cursor, self.visible().len());
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        if self.files.is_empty() {
            let note = Line::styled("No files changed", Style::default().fg(ui::DIM));
            frame.render_widget(note, area);
            return;
        }
        let selected_style = if focused {
            Style::default().bg(ui::SELECTED_BG).add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(ui::SELECTED_BG)
        };
        let lines: Vec<Line> = self
            .visible()
            .into_iter()
            .enumerate()
            .map(|(row_number, row)| {
                let selected = row_number == self.cursor;
                match row {
                    TreeRow::Folder { name, path, depth } => {
                        let arrow = if self.collapsed.contains(path) { '▶' } else { '▼' };
                        let indent = "  ".repeat(*depth);
                        let line = Line::styled(format!(" {indent}{arrow} {name}/"), Style::default().fg(ui::DIM));
                        if selected { line.style(selected_style) } else { line }
                    }
                    TreeRow::File { index, depth } => {
                        let file = &self.files[*index];
                        let indent = "  ".repeat(*depth);
                        styled_file_line(file, name_of(&file.path), None, indent, selected, focused)
                    }
                }
            })
            .collect();
        self.scroll = ui::keep_visible(self.scroll, self.cursor, area.height as usize);
        ui::render_lines(frame, area, lines.into_iter().skip(self.scroll).collect());
    }
}

/// A row of the tree: a folder, by the name shown and its whole path, or a
/// file by its index, each at a depth.
#[derive(Debug, PartialEq)]
pub enum TreeRow {
    Folder { name: String, path: String, depth: usize },
    File { index: usize, depth: usize },
}

impl TreeRow {
    fn depth(&self) -> usize {
        match self {
            TreeRow::Folder { depth, .. } | TreeRow::File { depth, .. } => *depth,
        }
    }
}

/// Folders and files, as a tree is built from paths.
#[derive(Default)]
struct Node {
    folders: BTreeMap<String, Node>,
    files: Vec<FileEntry>,
}

/// Orders files as a tree of their folders shows them, and returns its rows.
/// As in VS Code, folders come before files at each level, and a folder that
/// holds only one folder is shown on one line with it, like `docs/images/`.
fn tree(files: Vec<FileEntry>) -> (Vec<FileEntry>, Vec<TreeRow>) {
    let mut root = Node::default();
    for file in files {
        let mut node = &mut root;
        let mut parts: Vec<String> = file.path.split('/').map(str::to_string).collect();
        parts.pop(); // the file's own name
        for part in parts {
            node = node.folders.entry(part).or_default();
        }
        node.files.push(file);
    }
    let mut ordered = Vec::new();
    let mut rows = Vec::new();
    add_rows(&root, "", 0, &mut ordered, &mut rows);
    (ordered, rows)
}

fn add_rows(node: &Node, parent: &str, depth: usize, ordered: &mut Vec<FileEntry>, rows: &mut Vec<TreeRow>) {
    for (name, folder) in &node.folders {
        let (mut name, mut folder) = (name.clone(), folder);
        while folder.files.is_empty() && folder.folders.len() == 1 {
            let Some((inner_name, inner)) = folder.folders.iter().next() else { break };
            name = format!("{name}/{inner_name}");
            folder = inner;
        }
        let path = if parent.is_empty() { name.clone() } else { format!("{parent}/{name}") };
        rows.push(TreeRow::Folder { name, path: path.clone(), depth });
        add_rows(folder, &path, depth + 1, ordered, rows);
    }
    let mut files: Vec<&FileEntry> = node.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for file in files {
        rows.push(TreeRow::File { index: ordered.len(), depth });
        ordered.push(file.clone());
    }
}

/// The file's own name, the last part of its path.
fn name_of(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// The folder a file is in, if it's not at the top of the repo.
fn folder_of(path: &str) -> Option<&str> {
    path.trim_end_matches('/').rsplit_once('/').map(|(folder, _)| folder)
}

/// A file's row in a flat list, as VS Code lists changes: its name, then the
/// folder it's in, dimmed, like `app.rs  src/ui`.
pub fn file_line(file: &FileEntry, selected: bool, focused: bool) -> Line<'_> {
    styled_file_line(file, name_of(&file.path), folder_of(&file.path), String::new(), selected, focused)
}

/// A file's row: its status letter, `name`, and `folder` if given, dimmed,
/// after `indent`.
fn styled_file_line<'a>(
    file: &FileEntry,
    name: &'a str,
    folder: Option<&'a str>,
    indent: String,
    selected: bool,
    focused: bool,
) -> Line<'a> {
    let letter = Span::styled(
        format!(" {indent}{} ", file.status),
        Style::default().fg(ui::status_color(file.status)),
    );
    let mut spans = vec![letter, Span::raw(name)];
    if let Some(folder) = folder {
        spans.push(Span::styled(format!("  {folder}"), Style::default().fg(ui::DIM)));
    }
    let line = Line::from(spans);
    if selected && focused {
        line.style(Style::default().bg(ui::SELECTED_BG).add_modifier(Modifier::BOLD))
    } else if selected {
        line.style(Style::default().bg(ui::SELECTED_BG))
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<FileEntry> {
        paths.iter().map(|path| FileEntry { status: 'M', path: path.to_string() }).collect()
    }

    /// The tree's rows as text, indented by depth.
    fn drawn(paths: &[&str]) -> Vec<String> {
        let (ordered, rows) = tree(files(paths));
        rows.iter()
            .map(|row| match row {
                TreeRow::Folder { name, depth, .. } => format!("{}{name}/", "  ".repeat(*depth)),
                TreeRow::File { index, depth } => format!("{}{}", "  ".repeat(*depth), ordered[*index].path),
            })
            .collect()
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn files_nest_under_their_folders_with_folders_first() {
        let rows = drawn(&["README.md", "src/main.rs", "src/ui/app.rs", "src/ui/diff.rs", "Cargo.toml"]);
        assert_eq!(
            rows,
            ["src/", "  ui/", "    src/ui/app.rs", "    src/ui/diff.rs", "  src/main.rs", "Cargo.toml", "README.md"]
        );
    }

    #[test]
    fn a_folder_holding_only_a_folder_shares_its_line() {
        let rows = drawn(&["docs/images/a.svg", "docs/images/b.svg"]);
        assert_eq!(rows, ["docs/images/", "  docs/images/a.svg", "  docs/images/b.svg"]);
    }

    #[test]
    fn the_files_order_follows_the_tree() {
        let (ordered, _) = tree(files(&["z.md", "a/b.rs", "m.md"]));
        let paths: Vec<&str> = ordered.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["a/b.rs", "m.md", "z.md"]);
    }

    fn press(list: &mut FileList, ch: char) -> Action {
        list.handle_key(KeyEvent::from(KeyCode::Char(ch)))
    }

    /// The visible rows, as a folder's path or a file's path.
    fn shown(list: &FileList) -> Vec<String> {
        list.visible().into_iter().map(|row| list.row_key(row)).collect()
    }

    fn list(paths: &[&str]) -> FileList {
        FileList::new("a".to_string(), "b".to_string(), files(paths))
    }

    #[test]
    fn h_folds_a_folder_and_l_unfolds_it() {
        let mut list = list(&["src/a.rs", "src/b.rs", "z.md"]);
        assert_eq!(shown(&list), ["src/", "src/a.rs", "src/b.rs", "z.md"]);
        press(&mut list, 'h'); // from a.rs up to src/
        press(&mut list, 'h'); // and folded
        assert_eq!(shown(&list), ["src/", "z.md"]);
        press(&mut list, 'l');
        assert_eq!(shown(&list), ["src/", "src/a.rs", "src/b.rs", "z.md"]);
    }

    #[test]
    fn h_on_a_file_goes_up_to_its_folder_and_l_goes_in() {
        let mut list = list(&["src/a.rs", "src/b.rs"]);
        assert_eq!(list.cursor, 1); // the first file
        press(&mut list, 'j');
        assert_eq!(list.cursor, 2); // src/b.rs
        press(&mut list, 'h');
        assert_eq!(list.cursor, 0); // src/
        press(&mut list, 'l');
        assert_eq!(list.cursor, 1); // into src/, onto a.rs
    }

    #[test]
    fn folders_have_no_diff_and_files_keep_theirs() {
        let mut list = list(&["src/a.rs"]);
        assert!(matches!(list.selected_source(), Some(DiffSource::Refs { path, .. }) if path == "src/a.rs"));
        // On the folder, there's no diff, and the one showing stays.
        assert!(matches!(press(&mut list, 'k'), Action::None));
        assert!(list.selected_source().is_none());
    }

    #[test]
    fn a_list_that_was_empty_starts_on_its_first_file() {
        let mut list = list(&[]);
        list.set_files(files(&["src/a.rs"]));
        assert_eq!(shown(&list)[list.cursor], "src/a.rs");
    }

    #[test]
    fn a_refresh_keeps_folders_folded_and_the_cursor_on_its_file() {
        let mut list = list(&["docs/x.md", "src/a.rs", "src/b.rs"]);
        press(&mut list, 'h'); // from x.md up to docs/
        press(&mut list, 'h'); // and folded
        press(&mut list, 'j');
        press(&mut list, 'j');
        assert_eq!(shown(&list)[list.cursor], "src/a.rs");
        list.set_files(files(&["docs/x.md", "docs/y.md", "src/a.rs", "src/b.rs"]));
        assert_eq!(shown(&list), ["docs/", "src/", "src/a.rs", "src/b.rs"]);
        assert_eq!(shown(&list)[list.cursor], "src/a.rs");
    }

    #[test]
    fn a_flat_row_shows_the_name_then_its_folder() {
        let file = &files(&["src/ui/app.rs"])[0];
        assert_eq!(text(&file_line(file, false, false)), " M app.rs  src/ui");
        let top = &files(&["README.md"])[0];
        assert_eq!(text(&file_line(top, false, false)), " M README.md");
    }
}
