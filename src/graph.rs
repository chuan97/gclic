//! Feature 2: the commit chain. `layout` and `reach_masks` are pure functions
//! over commits; `GraphPanel` holds the state and draws them.

use crate::app::Action;
use crate::file_list::file_line;
use crate::git::{self, Commit, CommitDetails, DiffSource, FileEntry};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use std::collections::HashMap;

const COMMIT_LIMIT: usize = 500;
const PALETTE: [Color; 6] = [
    Color::Green,
    Color::LightBlue,
    Color::Magenta,
    Color::Yellow,
    Color::Cyan,
    Color::Red,
];

/// One character of the drawing. `commit` is the commit it leads to, whose
/// color it takes, so a line changes color where the history changes owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub ch: char,
    pub commit: Option<usize>,
}

#[derive(Debug, PartialEq)]
pub enum Row {
    /// `index` is the commit's position in the commits slice. `below` is the
    /// lines leaving this commit, drawn alongside its files when it is expanded.
    Commit { index: usize, glyphs: Vec<Glyph>, below: Vec<Glyph> },
    /// Lines between two commits: straight, forking or joining, e.g. `├─╯`.
    Connector { glyphs: Vec<Glyph> },
}

/// Assigns each commit a column and draws the lines between them.
///
/// `lanes` holds, per column, the sha that the line running through it is
/// heading for. Commits must be ordered with children before parents.
pub fn layout(commits: &[Commit]) -> Vec<Row> {
    let index_of: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| (commit.sha.as_str(), i))
        .collect();
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::new();
    // Whether a row of lines already separates the previous bead from the next.
    let mut separated = true;

    for (index, commit) in commits.iter().enumerate() {
        let sha = commit.sha.as_str();

        // Beads never touch: without a fork or join between them, add plain lines.
        let joins_here = lanes.iter().filter(|lane| **lane == Some(sha)).count() > 1;
        if !separated && !joins_here && lanes.iter().any(Option::is_some) {
            rows.push(Row::Connector { glyphs: lane_glyphs(&lanes, &index_of) });
        }
        separated = false;

        // A commit continues the leftmost line heading for it. With no such
        // line it is a branch tip and starts a new one.
        let column = lanes
            .iter()
            .position(|lane| *lane == Some(sha))
            .unwrap_or_else(|| free_lane(&mut lanes, 0));
        lanes[column] = Some(sha);

        // Any other lines heading for this commit join it here.
        for other in column + 1..lanes.len() {
            if lanes[other] == Some(sha) {
                rows.push(connector(&lanes, &index_of, column, other, '├', '╯', sha));
                lanes[other] = None;
            }
        }

        let mut glyphs = lane_glyphs(&lanes, &index_of);
        glyphs[column * 2] = Glyph { ch: '●', commit: Some(index) };

        // The first parent continues straight down.
        lanes[column] = commit.parents.first().map(String::as_str);
        let below = lane_glyphs(&lanes, &index_of);
        rows.push(Row::Commit { index, glyphs, below });

        // Merge commits send a line to each further parent.
        for parent in commit.parents.iter().skip(1) {
            separated = true;
            let parent = parent.as_str();
            match lanes.iter().position(|lane| *lane == Some(parent)) {
                Some(existing) if existing > column => {
                    rows.push(connector(&lanes, &index_of, column, existing, '├', '┤', parent));
                }
                Some(existing) if existing < column => {
                    rows.push(connector(&lanes, &index_of, column, existing, '┤', '├', parent));
                }
                Some(_) => {}
                None => {
                    let new = free_lane(&mut lanes, column + 1);
                    lanes[new] = Some(parent);
                    rows.push(connector(&lanes, &index_of, column, new, '├', '╮', parent));
                }
            }
        }

        while lanes.last() == Some(&None) {
            lanes.pop();
        }
    }
    rows
}

/// The first empty column at or after `from`, adding one if needed.
fn free_lane(lanes: &mut Vec<Option<&str>>, from: usize) -> usize {
    match (from..lanes.len()).find(|&i| lanes[i].is_none()) {
        Some(free) => free,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

/// A row with a vertical line for every active lane. Each column is two
/// characters wide: the lane itself, then a gap that connectors can cross.
fn lane_glyphs(lanes: &[Option<&str>], index_of: &HashMap<&str, usize>) -> Vec<Glyph> {
    let mut glyphs = Vec::with_capacity(lanes.len() * 2);
    for lane in lanes {
        match lane {
            Some(sha) => glyphs.push(Glyph { ch: '│', commit: index_of.get(*sha).copied() }),
            None => glyphs.push(Glyph { ch: ' ', commit: None }),
        }
        glyphs.push(Glyph { ch: ' ', commit: None });
    }
    glyphs
}

/// A row with a horizontal line between two columns.
fn connector(
    lanes: &[Option<&str>],
    index_of: &HashMap<&str, usize>,
    from: usize,
    to: usize,
    from_ch: char,
    to_ch: char,
    target: &str,
) -> Row {
    let commit = index_of.get(target).copied();
    let mut glyphs = lane_glyphs(lanes, index_of);
    let (left, right) = (from.min(to) * 2, from.max(to) * 2);
    for (x, glyph) in glyphs.iter_mut().enumerate().take(right).skip(left + 1) {
        let crossing = x % 2 == 0 && glyph.ch == '│';
        *glyph = Glyph { ch: if crossing { '┼' } else { '─' }, commit };
    }
    glyphs[from * 2] = Glyph { ch: from_ch, commit };
    glyphs[to * 2] = Glyph { ch: to_ch, commit };
    Row::Connector { glyphs }
}

/// For each commit, a bitmask of which tips can reach it (bit i = tips[i]).
/// One pass is enough because children come before their parents.
pub fn reach_masks(commits: &[Commit], tips: &[String]) -> Vec<u64> {
    let mut reach: HashMap<&str, u64> = HashMap::new();
    for (i, tip) in tips.iter().enumerate().take(64) {
        *reach.entry(tip.as_str()).or_default() |= 1 << i;
    }
    let mut masks = Vec::with_capacity(commits.len());
    for commit in commits {
        let mask = reach.get(commit.sha.as_str()).copied().unwrap_or(0);
        masks.push(mask);
        for parent in &commit.parents {
            *reach.entry(parent.as_str()).or_default() |= mask;
        }
    }
    masks
}

/// One color per ref, where refs with the same key share a color. The key is
/// the branch name without its remote, so `feature` and `origin/feature` match.
/// The exception is when one branch is all that's selected: telling it apart
/// from its remote is then the point of the view, so each ref gets its own color.
fn ref_colors(keys: &[String]) -> Vec<Color> {
    if keys.iter().all(|key| *key == keys[0]) {
        return (0..keys.len()).map(|i| PALETTE[i % PALETTE.len()]).collect();
    }
    let mut seen: Vec<&String> = Vec::new();
    keys.iter()
        .map(|key| {
            let position = seen.iter().position(|k| *k == key).unwrap_or_else(|| {
                seen.push(key);
                seen.len() - 1
            });
            PALETTE[position % PALETTE.len()]
        })
        .collect()
}

/// The color of each commit, given which refs reach it (see `reach_masks`).
/// Each commit takes the color of the first ref reaching it, trying refs with
/// no commits of their own first. So if `feature` is ahead of `origin/feature`,
/// the history they share takes the remote's color and only the commits ahead
/// take the local one, and the two colors always show.
fn commit_colors(masks: &[u64], colors: &[Color]) -> Vec<Color> {
    let has_own_commits = |bit: usize| masks.iter().any(|mask| *mask == 1 << bit);
    let mut order: Vec<usize> = (0..colors.len()).collect();
    // Stable, so refs in the same situation keep their selection order.
    order.sort_by_key(|&bit| has_own_commits(bit));
    masks
        .iter()
        .map(|mask| {
            let first = order.iter().find(|&&bit| mask & (1 << bit) != 0);
            first.map_or(PALETTE[0], |&bit| colors[bit])
        })
        .collect()
}

/// A named ref on screen and the commit it points at.
struct Tip {
    name: String,
    sha: String,
}

/// A commit opened as a drop-down, listing its changed files under it.
struct Expanded {
    sha: String,
    files: Vec<FileEntry>,
}

/// One row on screen.
enum Display {
    /// An index into `rows`.
    Graph(usize),
    /// A file of an expanded commit, as indexes into `commits` and its files.
    File { commit: usize, file: usize },
}

pub struct GraphPanel {
    /// Refs the user picked. Empty means the default view.
    custom_refs: Vec<String>,
    tips: Vec<Tip>,
    /// The color of each tip, in the same order.
    colors: Vec<Color>,
    /// Per commit, parallel to `commits`.
    commit_colors: Vec<Color>,
    unpushed: Vec<bool>,
    commits: Vec<Commit>,
    rows: Vec<Row>,
    /// Index into `commits`.
    selected: usize,
    scroll: usize,
    expanded: Vec<Expanded>,
    /// The file under the cursor, in the selected commit. None while the
    /// cursor is on the commit itself.
    selected_file: Option<usize>,
    details: Option<CommitDetails>,
}

impl GraphPanel {
    pub fn new() -> GraphPanel {
        GraphPanel {
            custom_refs: Vec::new(),
            tips: Vec::new(),
            colors: Vec::new(),
            commit_colors: Vec::new(),
            unpushed: Vec::new(),
            commits: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            expanded: Vec::new(),
            selected_file: None,
            details: None,
        }
    }

    pub fn custom_refs(&self) -> &[String] {
        &self.custom_refs
    }

    pub fn set_refs(&mut self, refs: Vec<String>) -> Result<()> {
        self.custom_refs = refs;
        self.expanded.clear();
        self.selected = 0;
        self.selected_file = None;
        self.refresh()
    }

    pub fn is_on_file(&self) -> bool {
        self.selected_file.is_some()
    }

    /// The drop-down of the commit at `index`, if it is open.
    fn expanded_at(&self, index: usize) -> Option<&Expanded> {
        let sha = &self.commits.get(index)?.sha;
        self.expanded.iter().find(|expanded| expanded.sha == *sha)
    }

    pub fn refresh(&mut self) -> Result<()> {
        let selected_sha = self.commits.get(self.selected).map(|c| c.sha.clone());
        // Browsing the top of the chain follows new commits as they arrive.
        // A selected file stays selected, along with the commit it belongs to.
        let follow_tip = self.selected == 0 && !self.is_on_file();
        let refs = if self.custom_refs.is_empty() {
            git::default_refs()
        } else {
            self.custom_refs.clone()
        };
        self.tips = refs
            .into_iter()
            .filter_map(|name| {
                let sha = git::resolve(&name)?;
                Some(Tip { name, sha })
            })
            .collect();

        let names: Vec<String> = self.tips.iter().map(|tip| tip.name.clone()).collect();
        let branches = git::branches()?;
        let color_keys: Vec<String> = names
            .iter()
            .map(|name| {
                let branch = branches.iter().find(|branch| branch.name == *name);
                branch.map_or(name.clone(), |branch| branch.local_name.clone())
            })
            .collect();
        self.colors = ref_colors(&color_keys);
        let shas: Vec<String> = self.tips.iter().map(|tip| tip.sha.clone()).collect();
        self.commits = if names.is_empty() {
            Vec::new()
        } else {
            git::log(&names, COMMIT_LIMIT)?
        };
        self.rows = layout(&self.commits);
        self.commit_colors = commit_colors(&reach_masks(&self.commits, &shas), &self.colors);
        self.unpushed = if names.is_empty() {
            Vec::new()
        } else {
            let unpushed = git::unpushed(&names)?;
            self.commits.iter().map(|commit| unpushed.contains(&commit.sha)).collect()
        };

        let commits = &self.commits;
        self.expanded
            .retain(|expanded| commits.iter().any(|commit| commit.sha == expanded.sha));

        if !follow_tip {
            let same_commit = selected_sha
                .and_then(|sha| self.commits.iter().position(|commit| commit.sha == sha));
            self.selected = same_commit
                .unwrap_or(self.selected)
                .min(self.commits.len().saturating_sub(1));
        }
        if self.expanded_at(self.selected).is_none() {
            self.selected_file = None;
        }
        self.update_details()
    }

    fn update_details(&mut self) -> Result<()> {
        if self.details.is_some() {
            self.details = match self.commits.get(self.selected) {
                Some(commit) => Some(git::commit_details(&commit.sha)?),
                None => None,
            };
        }
        Ok(())
    }

    fn toggle_details(&mut self) -> Result<()> {
        if self.details.take().is_none()
            && let Some(commit) = self.commits.get(self.selected) {
                self.details = Some(git::commit_details(&commit.sha)?);
            }
        Ok(())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        let Some(sha) = self.commits.get(self.selected).map(|c| c.sha.clone()) else {
            return Action::None;
        };
        match key.code {
            KeyCode::Char('y') => return Action::Copy(sha),
            KeyCode::Char('i') => return Action::after_view_change(self.toggle_details()),
            _ => {}
        }

        let last = self.commits.len() - 1;
        let before = self.selected;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('g') => self.select_commit(0),
            KeyCode::Char('G') => self.select_commit(last),
            KeyCode::Enter => return self.open_selection(sha),
            KeyCode::Esc => {
                self.collapse();
                return Action::None;
            }
            _ => return Action::None,
        }
        if self.selected != before
            && let Err(error) = self.update_details()
        {
            return Action::Error(error.to_string());
        }
        match self.selected_file_source() {
            Some(source) => Action::PreviewDiff(source),
            None => Action::None,
        }
    }

    fn select_commit(&mut self, index: usize) {
        self.selected = index;
        self.selected_file = None;
    }

    /// Closes the selected commit's drop-down. On a closed commit, closes them all.
    fn collapse(&mut self) {
        if self.expanded_at(self.selected).is_some() {
            self.remove_selected_drop_down();
        } else {
            self.expanded.clear();
        }
    }

    fn remove_selected_drop_down(&mut self) {
        let sha = self.commits[self.selected].sha.clone();
        self.expanded.retain(|expanded| expanded.sha != sha);
        self.selected_file = None;
    }

    /// Walks down through a commit's files, if it is expanded, then on to the next commit.
    fn move_down(&mut self) {
        let last = self.commits.len() - 1;
        if let Some(expanded) = self.expanded_at(self.selected) {
            let next = self.selected_file.map_or(0, |file| file + 1);
            if next < expanded.files.len() {
                self.selected_file = Some(next);
                return;
            }
            if self.selected == last {
                return; // already on the very last row
            }
        }
        self.select_commit((self.selected + 1).min(last));
    }

    /// The mirror of `move_down`: into the files of an expanded commit from below.
    fn move_up(&mut self) {
        if let Some(file) = self.selected_file {
            self.selected_file = file.checked_sub(1);
            return;
        }
        if self.selected == 0 {
            return;
        }
        self.select_commit(self.selected - 1);
        self.selected_file = self
            .expanded_at(self.selected)
            .and_then(|expanded| expanded.files.len().checked_sub(1));
    }

    fn selected_file_source(&self) -> Option<DiffSource> {
        let expanded = self.expanded_at(self.selected)?;
        let file = expanded.files.get(self.selected_file?)?;
        Some(DiffSource::Commit { sha: expanded.sha.clone(), path: file.path.clone() })
    }

    /// Enter: on a file opens its diff, on a commit toggles its drop-down.
    fn open_selection(&mut self, sha: String) -> Action {
        if let Some(source) = self.selected_file_source() {
            return Action::OpenDiff(source);
        }
        if self.expanded_at(self.selected).is_some() {
            self.remove_selected_drop_down();
            return Action::None;
        }
        match git::commit_files(&sha) {
            Ok(files) if files.is_empty() => {
                Action::Message("No files changed in this commit".to_string())
            }
            Ok(files) => {
                self.expanded.push(Expanded { sha, files });
                Action::None
            }
            Err(error) => Action::Error(error.to_string()),
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = ui::panel(" Graph ".to_string(), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let (main, details_area) = if self.details.is_some() {
            let [top, bottom] =
                Layout::vertical([Constraint::Min(3), Constraint::Length(9)]).areas(inner);
            (top, Some(bottom))
        } else {
            (inner, None)
        };

        self.render_chain(frame, main, focused);
        if let (Some(details), Some(details_area)) = (&self.details, details_area) {
            render_details(frame, details_area, details);
        }
    }

    fn render_chain(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        if self.commits.is_empty() {
            let note = Line::styled("No commits yet", Style::default().fg(ui::DIM));
            frame.render_widget(note, area);
            return;
        }
        let display = self.display_rows();
        let selected_line = display
            .iter()
            .position(|row| match row {
                Display::File { commit, file } => {
                    *commit == self.selected && self.selected_file == Some(*file)
                }
                Display::Graph(row) => {
                    self.selected_file.is_none() && self.is_row_of_selected_commit(*row)
                }
            })
            .unwrap_or(0);

        // Opening a drop-down shows as many of its files as fit.
        let height = area.height as usize;
        if self.selected_file.is_none()
            && let Some(expanded) = self.expanded_at(self.selected)
        {
            let files = expanded.files.len();
            self.scroll = ui::keep_visible(self.scroll, selected_line + files, height);
        }
        self.scroll = ui::keep_visible(self.scroll, selected_line, height);

        // Text starts at the same column on every row, so the chain stays clean.
        let graph_width = self
            .rows
            .iter()
            .map(|row| match row {
                Row::Commit { glyphs, .. } | Row::Connector { glyphs } => glyphs.len(),
            })
            .max()
            .unwrap_or(0);

        let mut lines: Vec<Line> = display
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|row| match row {
                Display::Graph(row) => self.row_line(&self.rows[*row], graph_width, focused),
                Display::File { commit, file } => {
                    self.file_row_line(*commit, *file, graph_width, focused)
                }
            })
            .collect();
        let at_end = self.scroll + lines.len() == display.len();
        if at_end && self.commits.len() == COMMIT_LIMIT {
            lines.push(Line::styled("┊", Style::default().fg(ui::DIM)));
        }
        ui::render_lines(frame, area, lines);
    }

    /// The graph's rows, with each expanded commit's files inserted under it.
    fn display_rows(&self) -> Vec<Display> {
        let mut display = Vec::with_capacity(self.rows.len());
        for (i, row) in self.rows.iter().enumerate() {
            display.push(Display::Graph(i));
            if let Row::Commit { index, .. } = row
                && let Some(expanded) = self.expanded_at(*index)
            {
                let commit = *index;
                display.extend((0..expanded.files.len()).map(|file| Display::File { commit, file }));
            }
        }
        display
    }

    fn is_row_of_selected_commit(&self, row: usize) -> bool {
        matches!(self.rows[row], Row::Commit { index, .. } if index == self.selected)
    }

    fn glyph_span(&self, glyph: &Glyph) -> Span<'static> {
        let color = glyph.commit.map_or(PALETTE[0], |i| self.commit_colors[i]);
        let hollow = glyph.ch == '●' && glyph.commit.is_some_and(|i| self.unpushed[i]);
        let ch = if hollow { '○' } else { glyph.ch };
        Span::styled(ch.to_string(), Style::default().fg(color))
    }

    fn row_line(&self, row: &Row, graph_width: usize, focused: bool) -> Line<'_> {
        match row {
            Row::Connector { glyphs } => {
                Line::from(glyphs.iter().map(|g| self.glyph_span(g)).collect::<Vec<_>>())
            }
            Row::Commit { index, glyphs, .. } => {
                let selected = *index == self.selected && !self.is_on_file();
                let mut spans: Vec<Span> = glyphs.iter().map(|g| self.glyph_span(g)).collect();
                spans.push(Span::raw(" ".repeat(graph_width - glyphs.len() + 1)));

                let commit = &self.commits[*index];
                let subject = if selected && focused { Color::White } else { Color::Gray };
                spans.push(Span::styled(commit.subject.as_str(), Style::default().fg(subject)));
                spans.push(Span::styled(
                    format!("  {}", commit.author),
                    Style::default().fg(ui::DIM),
                ));
                for (tip, color) in self.tips.iter().zip(&self.colors) {
                    if tip.sha == commit.sha {
                        let style = Style::default().fg(*color).add_modifier(Modifier::BOLD);
                        spans.push(Span::raw("  "));
                        spans.push(Span::styled(tip.name.as_str(), style));
                    }
                }

                let line = Line::from(spans);
                if selected {
                    line.style(Style::default().bg(ui::SELECTED_BG))
                } else {
                    line
                }
            }
        }
    }

    /// A file in the drop-down, with the graph's lines carrying on beside it.
    fn file_row_line(
        &self,
        commit: usize,
        file: usize,
        graph_width: usize,
        focused: bool,
    ) -> Line<'_> {
        let Some(expanded) = self.expanded_at(commit) else {
            return Line::default();
        };
        let below = self.rows.iter().find_map(|row| match row {
            Row::Commit { index, below, .. } if *index == commit => Some(below.as_slice()),
            _ => None,
        });
        let below = below.unwrap_or_default();

        let mut spans: Vec<Span> = below.iter().map(|g| self.glyph_span(g)).collect();
        spans.push(Span::raw(" ".repeat(graph_width.saturating_sub(below.len()) + 1)));
        let selected = commit == self.selected && self.selected_file == Some(file);
        let mut line = file_line(&expanded.files[file], selected, focused);
        spans.append(&mut line.spans);
        line.spans = spans;
        line
    }
}

fn render_details(frame: &mut Frame, area: Rect, details: &CommitDetails) {
    let label = Style::default().fg(ui::DIM);
    let mut lines = vec![
        Line::from(vec![Span::styled("sha    ", label), Span::raw(details.sha.as_str())]),
        Line::from(vec![Span::styled("author ", label), Span::raw(details.author.as_str())]),
        Line::from(vec![Span::styled("date   ", label), Span::raw(details.date.as_str())]),
        Line::default(),
    ];
    lines.extend(details.message.lines().map(Line::raw));
    let block = ui::panel(" Commit ".to_string(), false);
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: &str, parents: &[&str]) -> Commit {
        Commit {
            sha: sha.to_string(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            subject: String::new(),
        }
    }

    /// Renders rows as text so tests can compare against a picture.
    fn draw(commits: &[Commit]) -> Vec<String> {
        layout(commits)
            .iter()
            .map(|row| {
                let (Row::Commit { glyphs, .. } | Row::Connector { glyphs }) = row;
                let text: String = glyphs.iter().map(|g| g.ch).collect();
                text.trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn linear_history_is_beads_joined_by_lines() {
        let commits = [commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])];
        assert_eq!(draw(&commits), ["●", "│", "●", "│", "●"]);
    }

    #[test]
    fn diverged_branches_join_above_their_common_ancestor() {
        let commits = [
            commit("l2", &["l1"]),
            commit("l1", &["base"]),
            commit("r1", &["base"]),
            commit("base", &["root"]),
            commit("root", &[]),
        ];
        assert_eq!(draw(&commits), ["●", "│", "●", "│", "│ ●", "├─╯", "●", "│", "●"]);
    }

    #[test]
    fn merge_forks_then_rejoins() {
        let commits = [
            commit("m", &["p1", "p2"]),
            commit("p1", &["base"]),
            commit("p2", &["base"]),
            commit("base", &[]),
        ];
        assert_eq!(draw(&commits), ["●", "├─╮", "● │", "│ │", "│ ●", "├─╯", "●"]);
    }

    #[test]
    fn merge_into_an_existing_lane_uses_a_tee() {
        // tip's line heads for `shared`, which is also the merge's second parent.
        let commits = [
            commit("tip", &["shared"]),
            commit("m", &["first", "shared"]),
            commit("first", &["shared"]),
            commit("shared", &[]),
        ];
        assert_eq!(draw(&commits), ["●", "│", "│ ●", "├─┤", "│ ●", "├─╯", "●"]);
    }

    #[test]
    fn three_tips_join_one_at_a_time() {
        let commits = [
            commit("a", &["base"]),
            commit("b", &["base"]),
            commit("c", &["base"]),
            commit("base", &[]),
        ];
        assert_eq!(draw(&commits), ["●", "│", "│ ●", "│ │", "│ │ ●", "├─╯ │", "├───╯", "●"]);
    }

    #[test]
    fn connector_passing_over_a_lane_draws_a_crossing() {
        let commits = [
            commit("a", &["merge"]),
            commit("b", &["other"]),
            commit("c", &["far"]),
            commit("merge", &["first", "far"]),
        ];
        assert_eq!(draw(&commits).last().unwrap(), "├─┼─┤");
    }

    #[test]
    fn a_finished_history_frees_its_lane() {
        let commits = [commit("a", &[]), commit("b", &[])];
        assert_eq!(draw(&commits), ["●", "●"]);
    }

    #[test]
    fn masks_separate_local_remote_and_shared_commits() {
        let commits = [
            commit("local", &["base"]),
            commit("remote", &["base"]),
            commit("base", &["root"]),
            commit("root", &[]),
        ];
        let masks = reach_masks(&commits, &["local".to_string(), "remote".to_string()]);
        assert_eq!(masks, [0b01, 0b10, 0b11, 0b11]);
    }

    const LOCAL: Color = PALETTE[0];
    const REMOTE: Color = PALETTE[1];

    #[test]
    fn local_ahead_shows_shared_history_in_the_remote_color() {
        // A commit only on local, then history on both.
        assert_eq!(commit_colors(&[0b01, 0b11], &[LOCAL, REMOTE]), [LOCAL, REMOTE]);
    }

    #[test]
    fn remote_ahead_shows_shared_history_in_the_local_color() {
        // A commit only on the remote, then history on both.
        assert_eq!(commit_colors(&[0b10, 0b11], &[LOCAL, REMOTE]), [REMOTE, LOCAL]);
    }

    #[test]
    fn diverged_shows_shared_history_in_the_first_color() {
        let colors = commit_colors(&[0b01, 0b10, 0b11], &[LOCAL, REMOTE]);
        assert_eq!(colors, [LOCAL, REMOTE, LOCAL]);
    }

    #[test]
    fn in_sync_is_all_the_first_color() {
        assert_eq!(commit_colors(&[0b11, 0b11], &[LOCAL, REMOTE]), [LOCAL, LOCAL]);
    }

    #[test]
    fn a_lone_branch_and_its_remote_get_different_colors() {
        let keys = ["feature", "feature"].map(str::to_string);
        assert_eq!(ref_colors(&keys), [PALETTE[0], PALETTE[1]]);
    }

    #[test]
    fn no_refs_means_no_colors() {
        assert!(ref_colors(&[]).is_empty());
    }

    #[test]
    fn a_branch_and_its_remotes_share_a_color() {
        let keys = ["feature", "main", "feature"].map(str::to_string);
        let colors = ref_colors(&keys);
        assert_eq!(colors, [PALETTE[0], PALETTE[1], PALETTE[0]]);
    }

    #[test]
    fn lines_take_the_color_of_the_commit_they_lead_to() {
        let commits = [commit("a", &["base"]), commit("b", &["base"]), commit("base", &[])];
        let rows = layout(&commits);
        let Row::Connector { glyphs } = &rows[3] else { panic!("expected a connector") };
        assert_eq!(glyphs[0].ch, '├');
        assert_eq!(glyphs[0].commit, Some(2));
    }

    #[test]
    fn plain_lines_take_the_color_of_the_parent() {
        let commits = [commit("child", &["parent"]), commit("parent", &[])];
        let rows = layout(&commits);
        let Row::Connector { glyphs } = &rows[1] else { panic!("expected a connector") };
        assert_eq!(glyphs[0].commit, Some(1));
    }

    #[test]
    fn lines_below_a_commit_are_kept_for_its_drop_down() {
        let commits = [commit("a", &["b"]), commit("b", &[])];
        let rows = layout(&commits);
        let Row::Commit { below, .. } = &rows[0] else { panic!("expected a commit") };
        assert_eq!(below[0].ch, '│');
        let Row::Commit { below, .. } = &rows[2] else { panic!("expected a commit") };
        assert_eq!(below[0].ch, ' ');
    }

    /// A panel showing `shas` as a linear history, with the given commits expanded.
    fn panel_with_expanded(shas: &[&str], open: &[&str], files: &[&str]) -> GraphPanel {
        let mut panel = GraphPanel::new();
        panel.commits = shas.iter().map(|sha| commit(sha, &[])).collect();
        for sha in open {
            panel.expanded.push(Expanded {
                sha: sha.to_string(),
                files: files
                    .iter()
                    .map(|path| FileEntry { status: 'M', path: path.to_string() })
                    .collect(),
            });
        }
        panel
    }

    fn panel_with_expanded_first_commit(shas: &[&str], files: &[&str]) -> GraphPanel {
        panel_with_expanded(shas, &shas[..1], files)
    }

    #[test]
    fn the_cursor_walks_through_every_open_drop_down() {
        let mut panel = panel_with_expanded(&["a", "b", "c"], &["a", "b"], &["x"]);
        panel.move_down();
        assert_eq!((panel.selected, panel.selected_file), (0, Some(0)));
        panel.move_down();
        assert_eq!((panel.selected, panel.selected_file), (1, None));
        panel.move_down();
        assert_eq!((panel.selected, panel.selected_file), (1, Some(0)));
        panel.move_down();
        assert_eq!((panel.selected, panel.selected_file), (2, None));
    }

    #[test]
    fn esc_collapses_only_the_selected_commit() {
        let mut panel = panel_with_expanded(&["a", "b", "c"], &["a", "b"], &["x"]);
        panel.selected = 1;
        panel.selected_file = Some(0);
        panel.collapse();
        assert_eq!(panel.selected_file, None);
        assert_eq!(panel.selected, 1);
        assert!(panel.expanded_at(0).is_some());
        assert!(panel.expanded_at(1).is_none());
    }

    #[test]
    fn esc_on_a_closed_commit_collapses_everything() {
        let mut panel = panel_with_expanded(&["a", "b", "c"], &["a", "b"], &["x"]);
        panel.selected = 2;
        panel.collapse();
        assert!(panel.expanded.is_empty());
    }

    #[test]
    fn moving_down_walks_through_the_files_then_to_the_next_commit() {
        let mut panel = panel_with_expanded_first_commit(&["a", "b"], &["x", "y"]);
        panel.move_down();
        assert_eq!(panel.selected_file, Some(0));
        panel.move_down();
        assert_eq!(panel.selected_file, Some(1));
        panel.move_down();
        assert_eq!(panel.selected, 1);
        assert_eq!(panel.selected_file, None);
    }

    #[test]
    fn moving_up_enters_the_files_from_below() {
        let mut panel = panel_with_expanded_first_commit(&["a", "b"], &["x", "y"]);
        panel.selected = 1;
        panel.move_up();
        assert_eq!(panel.selected, 0);
        assert_eq!(panel.selected_file, Some(1));
        panel.move_up();
        panel.move_up();
        assert_eq!(panel.selected_file, None);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn moving_past_the_last_file_of_the_last_commit_stays_put() {
        let mut panel = panel_with_expanded_first_commit(&["a"], &["x"]);
        panel.move_down();
        panel.move_down();
        assert_eq!(panel.selected_file, Some(0));
    }

    #[test]
    fn a_selected_file_points_its_diff_at_the_expanded_commit() {
        let mut panel = panel_with_expanded_first_commit(&["a", "b"], &["x"]);
        assert_eq!(panel.selected_file_source(), None);
        panel.move_down();
        let expected = DiffSource::Commit { sha: "a".to_string(), path: "x".to_string() };
        assert_eq!(panel.selected_file_source(), Some(expected));
    }
}
