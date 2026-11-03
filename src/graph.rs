//! Feature 2: the commit chain. `layout` and `reach_masks` are pure functions
//! over commits; `GraphPanel` holds the state and draws them.

use crate::app::Action;
use crate::file_list::{FileList, Origin};
use crate::git::{self, Commit, CommitDetails};
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
    /// `index` is the commit's position in the commits slice.
    Commit { index: usize, glyphs: Vec<Glyph> },
    /// Lines forking or joining between two commits, e.g. `├─╯`.
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

    for (index, commit) in commits.iter().enumerate() {
        let sha = commit.sha.as_str();

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
        rows.push(Row::Commit { index, glyphs });

        // The first parent continues straight down.
        lanes[column] = commit.parents.first().map(String::as_str);

        // Merge commits send a line to each further parent.
        for parent in commit.parents.iter().skip(1) {
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

/// Reached by every ref: shared, so dim. Reached by exactly one: that ref's
/// color. In between: neutral.
fn color_for(mask: u64, ref_count: usize) -> Color {
    if ref_count <= 1 {
        return PALETTE[0];
    }
    match mask.count_ones() as usize {
        1 => PALETTE[mask.trailing_zeros() as usize % PALETTE.len()],
        n if n >= ref_count => ui::DIM,
        _ => Color::Gray,
    }
}

/// A named ref on screen and the commit it points at.
struct Tip {
    name: String,
    sha: String,
}

pub struct GraphPanel {
    /// Refs the user picked. Empty means the default view.
    custom_refs: Vec<String>,
    tips: Vec<Tip>,
    commits: Vec<Commit>,
    rows: Vec<Row>,
    masks: Vec<u64>,
    /// Index into `commits`.
    selected: usize,
    scroll: usize,
    /// The changed files of the opened commit, shown in place of the chain.
    files: Option<FileList>,
    details: Option<CommitDetails>,
}

impl GraphPanel {
    pub fn new() -> GraphPanel {
        GraphPanel {
            custom_refs: Vec::new(),
            tips: Vec::new(),
            commits: Vec::new(),
            rows: Vec::new(),
            masks: Vec::new(),
            selected: 0,
            scroll: 0,
            files: None,
            details: None,
        }
    }

    pub fn custom_refs(&self) -> &[String] {
        &self.custom_refs
    }

    pub fn set_refs(&mut self, refs: Vec<String>) -> Result<()> {
        self.custom_refs = refs;
        self.files = None;
        self.selected = 0;
        self.refresh()
    }

    pub fn is_showing_files(&self) -> bool {
        self.files.is_some()
    }

    pub fn refresh(&mut self) -> Result<()> {
        let selected_sha = self.commits.get(self.selected).map(|c| c.sha.clone());
        // Browsing the top of the chain follows new commits as they arrive.
        // An opened commit stays selected, as its file list belongs to it.
        let follow_tip = self.selected == 0 && self.files.is_none();
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
        let shas: Vec<String> = self.tips.iter().map(|tip| tip.sha.clone()).collect();
        self.commits = if names.is_empty() {
            Vec::new()
        } else {
            git::log(&names, COMMIT_LIMIT)?
        };
        self.rows = layout(&self.commits);
        self.masks = reach_masks(&self.commits, &shas);

        if !follow_tip {
            let same_commit = selected_sha
                .and_then(|sha| self.commits.iter().position(|commit| commit.sha == sha));
            self.selected = same_commit
                .unwrap_or(self.selected)
                .min(self.commits.len().saturating_sub(1));
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
        if let Some(files) = &mut self.files {
            if key.code == KeyCode::Esc {
                self.files = None;
                return Action::None;
            }
            return files.handle_key(key);
        }

        let last = self.commits.len() - 1;
        let before = self.selected;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('g') => self.selected = 0,
            KeyCode::Char('G') => self.selected = last,
            KeyCode::Enter => {
                return match git::commit_files(&sha) {
                    Ok(files) => {
                        self.files = Some(FileList::new(Origin::Commit { sha }, files));
                        Action::None
                    }
                    Err(error) => Action::Error(error.to_string()),
                };
            }
            _ => {}
        }
        if self.selected != before {
            return Action::after_view_change(self.update_details());
        }
        Action::None
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let title = match self.files {
            Some(_) => format!(" Files in {} ", self.selected_short_sha()),
            None => " Graph ".to_string(),
        };
        let block = ui::panel(title, focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let (main, details_area) = if self.details.is_some() {
            let [top, bottom] =
                Layout::vertical([Constraint::Min(3), Constraint::Length(9)]).areas(inner);
            (top, Some(bottom))
        } else {
            (inner, None)
        };

        if let Some(files) = &mut self.files {
            files.render(frame, main, focused);
        } else {
            self.render_chain(frame, main, focused);
        }
        if let (Some(details), Some(details_area)) = (&self.details, details_area) {
            render_details(frame, details_area, details);
        }
    }

    fn selected_short_sha(&self) -> &str {
        self.commits
            .get(self.selected)
            .map_or("", |commit| &commit.sha[..commit.sha.len().min(7)])
    }

    fn render_chain(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        if self.commits.is_empty() {
            let note = Line::styled("No commits yet", Style::default().fg(ui::DIM));
            frame.render_widget(note, area);
            return;
        }
        let selected_row = self
            .rows
            .iter()
            .position(|row| matches!(row, Row::Commit { index, .. } if *index == self.selected))
            .unwrap_or(0);
        self.scroll = ui::keep_visible(self.scroll, selected_row, area.height as usize);

        // Text starts at the same column on every row, so the chain stays clean.
        let graph_width = self
            .rows
            .iter()
            .map(|row| match row {
                Row::Commit { glyphs, .. } | Row::Connector { glyphs } => glyphs.len(),
            })
            .max()
            .unwrap_or(0);

        let mut lines: Vec<Line> = self
            .rows
            .iter()
            .skip(self.scroll)
            .take(area.height as usize)
            .map(|row| self.row_line(row, graph_width, focused))
            .collect();
        let at_end = self.scroll + lines.len() == self.rows.len();
        if at_end && self.commits.len() == COMMIT_LIMIT {
            lines.push(Line::styled("┊", Style::default().fg(ui::DIM)));
        }
        ui::render_lines(frame, area, lines);
    }

    fn row_line(&self, row: &Row, graph_width: usize, focused: bool) -> Line<'_> {
        let ref_count = self.tips.len();
        let glyph_span = |glyph: &Glyph| {
            let color = glyph
                .commit
                .map_or(ui::DIM, |i| color_for(self.masks[i], ref_count));
            Span::styled(glyph.ch.to_string(), Style::default().fg(color))
        };
        match row {
            Row::Connector { glyphs } => Line::from(glyphs.iter().map(glyph_span).collect::<Vec<_>>()),
            Row::Commit { index, glyphs } => {
                let selected = *index == self.selected;
                let mut spans: Vec<Span> = glyphs.iter().map(glyph_span).collect();
                spans.push(Span::raw(" ".repeat(graph_width - glyphs.len() + 1)));

                let commit = &self.commits[*index];
                for (bit, tip) in self.tips.iter().enumerate() {
                    if tip.sha == commit.sha {
                        let color = PALETTE[bit % PALETTE.len()];
                        let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                        spans.push(Span::styled(tip.name.as_str(), style));
                        spans.push(Span::raw(" "));
                    }
                }
                let subject = if selected && focused { Color::White } else { Color::Gray };
                spans.push(Span::styled(commit.subject.as_str(), Style::default().fg(subject)));

                let line = Line::from(spans);
                if selected {
                    line.style(Style::default().bg(ui::SELECTED_BG))
                } else {
                    line
                }
            }
        }
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
    fn linear_history_is_one_straight_chain() {
        let commits = [commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])];
        assert_eq!(draw(&commits), ["●", "●", "●"]);
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
        assert_eq!(draw(&commits), ["●", "●", "│ ●", "├─╯", "●", "●"]);
    }

    #[test]
    fn merge_forks_then_rejoins() {
        let commits = [
            commit("m", &["p1", "p2"]),
            commit("p1", &["base"]),
            commit("p2", &["base"]),
            commit("base", &[]),
        ];
        assert_eq!(draw(&commits), ["●", "├─╮", "● │", "│ ●", "├─╯", "●"]);
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
        assert_eq!(draw(&commits), ["●", "│ ●", "├─┤", "│ ●", "├─╯", "●"]);
    }

    #[test]
    fn three_tips_join_one_at_a_time() {
        let commits = [
            commit("a", &["base"]),
            commit("b", &["base"]),
            commit("c", &["base"]),
            commit("base", &[]),
        ];
        assert_eq!(draw(&commits), ["●", "│ ●", "│ │ ●", "├─╯ │", "├───╯", "●"]);
    }

    #[test]
    fn connector_passing_over_a_lane_draws_a_crossing() {
        let commits = [
            commit("a", &["merge"]),
            commit("b", &["other"]),
            commit("c", &["far"]),
            commit("merge", &["first", "far"]),
        ];
        assert_eq!(draw(&commits)[4], "├─┼─┤");
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

    #[test]
    fn colors_dim_shared_commits_only() {
        assert_eq!(color_for(0b01, 2), PALETTE[0]);
        assert_eq!(color_for(0b10, 2), PALETTE[1]);
        assert_eq!(color_for(0b11, 2), ui::DIM);
        assert_eq!(color_for(0b011, 3), Color::Gray);
        assert_eq!(color_for(0b1, 1), PALETTE[0]);
    }

    #[test]
    fn lines_take_the_color_of_the_commit_they_lead_to() {
        let commits = [commit("a", &["base"]), commit("b", &["base"]), commit("base", &[])];
        let rows = layout(&commits);
        let Row::Connector { glyphs } = &rows[2] else { panic!("expected a connector") };
        assert_eq!(glyphs[0].commit, Some(2));
    }
}
