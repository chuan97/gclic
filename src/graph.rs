//! Feature 2: the commit chain. `layout` and `reach_masks` are pure functions
//! over commits; `GraphPanel` holds the state and draws them.

use crate::app::Action;
use crate::file_list::file_line;
use crate::git::{self, Branch, Commit, DiffSource, FileEntry};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::{HashMap, HashSet};
use std::thread;

/// Also read by the app, which writes key marks right after it.
pub const TITLE: &str = " Graph ";

const COMMIT_LIMIT: usize = 500;
/// The number of refs that fit in the bitmask `reach_masks` builds.
const MAX_MASK_BITS: usize = 64;
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

/// The drawing for one commit. `layout` returns one per commit, in the same order.
#[derive(Debug, PartialEq)]
pub struct Row {
    /// The bead, the lines passing it, and any lines forking or joining at it.
    pub glyphs: Vec<Glyph>,
    /// The lines leaving this commit, drawn alongside its files when it is expanded.
    pub below: Vec<Glyph>,
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
    let mut rows = Vec::with_capacity(commits.len());

    for (index, commit) in commits.iter().enumerate() {
        let sha = commit.sha.as_str();

        // A commit continues the leftmost line heading for it. With no such
        // line it is a branch tip and starts a new one.
        let column = lanes
            .iter()
            .position(|lane| *lane == Some(sha))
            .unwrap_or_else(|| free_lane(&mut lanes, 0));
        let above = lanes.clone();

        // The lines reaching the bead sideways, as their column and the commit
        // they lead to. First, any other lines heading for this commit join it.
        let mut sideways: Vec<(usize, Option<usize>)> = (column + 1..lanes.len())
            .filter(|&other| lanes[other] == Some(sha))
            .map(|other| (other, Some(index)))
            .collect();

        // The first parent continues straight down.
        lanes[column] = commit.parents.first().map(String::as_str);

        // Merge commits send a line to each further parent.
        for parent in commit.parents.iter().skip(1) {
            let parent = parent.as_str();
            let lane = lanes.iter().position(|lane| *lane == Some(parent)).unwrap_or_else(|| {
                let new = free_lane(&mut lanes, column + 1);
                lanes[new] = Some(parent);
                new
            });
            if lane != column {
                sideways.push((lane, index_of.get(parent).copied()));
            }
        }

        // Only freed now, as a new line starting in a column where another ends
        // would look like a single line running through.
        for lane in lanes.iter_mut().filter(|lane| **lane == Some(sha)) {
            *lane = None;
        }

        let glyphs = commit_glyphs(index, column, &above, &lanes, sideways, &index_of);
        rows.push(Row { glyphs, below: lane_glyphs(&lanes, &index_of) });

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

/// A commit's row: its bead, with `above` and `below` being the lanes on either
/// side of it and `sideways` the lines running horizontally from it.
fn commit_glyphs(
    index: usize,
    column: usize,
    above: &[Option<&str>],
    below: &[Option<&str>],
    mut sideways: Vec<(usize, Option<usize>)>,
    index_of: &HashMap<&str, usize>,
) -> Vec<Glyph> {
    let is_active = |lanes: &[Option<&str>], lane: usize| lanes.get(lane).is_some_and(|l| l.is_some());

    // New lanes only exist below, the ones that just ended only above.
    let lanes: Vec<Option<&str>> = (0..above.len().max(below.len()))
        .map(|lane| below.get(lane).copied().flatten().or(above.get(lane).copied().flatten()))
        .collect();
    let mut glyphs = lane_glyphs(&lanes, index_of);

    // The farthest go first, so where lines overlap each cell takes the color
    // of the nearest one, and every line shows up to where it ends.
    sideways.sort_by_key(|(lane, _)| std::cmp::Reverse(lane.abs_diff(column)));
    for &(lane, commit) in &sideways {
        let (from, to) = (lane.min(column) * 2, lane.max(column) * 2);
        for (x, glyph) in glyphs.iter_mut().enumerate().take(to + 1).skip(from) {
            glyph.commit = commit;
            if x % 2 == 1 {
                glyph.ch = '─';
            }
        }
    }

    let reach = |pick: fn(usize, usize) -> usize| {
        sideways.iter().fold(column, |far, (lane, _)| pick(far, *lane))
    };
    let (left, right) = (reach(usize::min), reach(usize::max));
    for lane in left..=right {
        let (up, down) = (is_active(above, lane), is_active(below, lane));
        glyphs[lane * 2].ch = box_char(up, down, lane > left, lane < right);
    }
    let ch = bead_char(
        is_active(above, column),
        is_active(below, column),
        left < column,
        right > column,
    );
    glyphs[column * 2] = Glyph { ch, commit: Some(index) };
    glyphs
}

/// The filled bead whose lines reach the given edges of its cell, so that beads
/// on consecutive rows are joined by an unbroken line. These are the "branch
/// drawing" symbols in the Private Use Area, which kitty introduced for git
/// graphs: U+F5EE to U+F60D, alternating filled and hollow. Ghostty and kitty
/// draw them themselves, but other terminals need a font that has them.
fn bead_char(up: bool, down: bool, left: bool, right: bool) -> char {
    match (up, down, left, right) {
        (false, false, false, false) => '\u{f5ee}',
        (false, false, false, true) => '\u{f5f0}',
        (false, false, true, false) => '\u{f5f2}',
        (false, false, true, true) => '\u{f5f4}',
        (false, true, false, false) => '\u{f5f6}',
        (true, false, false, false) => '\u{f5f8}',
        (true, true, false, false) => '\u{f5fa}',
        (false, true, false, true) => '\u{f5fc}',
        (false, true, true, false) => '\u{f5fe}',
        (true, false, false, true) => '\u{f600}',
        (true, false, true, false) => '\u{f602}',
        (true, true, false, true) => '\u{f604}',
        (true, true, true, false) => '\u{f606}',
        (false, true, true, true) => '\u{f608}',
        (true, false, true, true) => '\u{f60a}',
        (true, true, true, true) => '\u{f60c}',
    }
}

fn is_bead(ch: char) -> bool {
    ('\u{f5ee}'..='\u{f60d}').contains(&ch)
}

/// The hollow bead that comes right after each filled one in `bead_char`.
fn hollow(bead: char) -> char {
    char::from_u32(bead as u32 + 1).unwrap_or(bead)
}

/// The box-drawing character with arms in the given directions.
fn box_char(up: bool, down: bool, left: bool, right: bool) -> char {
    match (up, down, left, right) {
        (true, true, true, true) => '┼',
        (true, true, true, false) => '┤',
        (true, true, false, true) => '├',
        (true, false, true, true) => '┴',
        (false, true, true, true) => '┬',
        (true, false, true, false) => '╯',
        (true, false, false, true) => '╰',
        (false, true, true, false) => '╮',
        (false, true, false, true) => '╭',
        (_, _, true, _) | (_, _, _, true) => '─',
        (true, _, _, _) | (_, true, _, _) => '│',
        _ => ' ',
    }
}

/// For each commit, a bitmask of which tips can reach it (bit i = tips[i]).
/// One pass is enough because children come before their parents.
pub fn reach_masks(commits: &[Commit], tips: &[String]) -> Vec<u64> {
    let mut reach: HashMap<&str, u64> = HashMap::new();
    for (i, tip) in tips.iter().enumerate().take(MAX_MASK_BITS) {
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
    // Masks only have room for the first MAX_MASK_BITS refs.
    let mut order: Vec<usize> = (0..colors.len().min(MAX_MASK_BITS)).collect();
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
    /// An index into `commits`.
    Commit(usize),
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
        }
    }

    pub fn custom_refs(&self) -> &[String] {
        &self.custom_refs
    }

    pub fn set_refs(&mut self, refs: Vec<String>, branches: &[Branch]) -> Result<()> {
        self.custom_refs = refs;
        self.expanded.clear();
        self.selected = 0;
        self.selected_file = None;
        let result = self.refresh();
        self.color_by(branches);
        result
    }

    pub fn is_on_file(&self) -> bool {
        self.selected_file.is_some()
    }

    /// The commit under the cursor, or the one its file belongs to.
    pub fn selected_sha(&self) -> Option<&str> {
        self.commits.get(self.selected).map(|commit| commit.sha.as_str())
    }

    pub fn is_selected_expanded(&self) -> bool {
        self.expanded_at(self.selected).is_some()
    }

    pub fn has_expanded(&self) -> bool {
        !self.expanded.is_empty()
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
        let shas = git::resolve_all(&refs);
        self.tips = refs
            .into_iter()
            .zip(shas)
            .filter_map(|(name, sha)| Some(Tip { name, sha: sha? }))
            .collect();

        let names: Vec<String> = self.tips.iter().map(|tip| tip.name.clone()).collect();
        let (commits, unpushed) = if names.is_empty() {
            (Vec::new(), HashSet::new())
        } else {
            // Two separate git calls, so they run at once.
            thread::scope(|scope| {
                let unpushed = scope.spawn(|| git::unpushed(&names));
                let commits = git::log(&names, COMMIT_LIMIT);
                let unpushed = unpushed.join().unwrap_or_else(|_| Err(anyhow::anyhow!("git crashed")));
                Ok::<_, anyhow::Error>((commits?, unpushed?))
            })?
        };
        self.commits = commits;
        self.rows = layout(&self.commits);
        self.unpushed = self.commits.iter().map(|commit| unpushed.contains(&commit.sha)).collect();

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
        Ok(())
    }

    /// Colours the refs and commits, which needs the list of branches: a
    /// branch and its remotes share a colour. Kept apart from `refresh` so the
    /// list is fetched once, by the Branches pane, while the graph loads.
    pub fn color_by(&mut self, branches: &[Branch]) {
        let color_keys: Vec<String> = self
            .tips
            .iter()
            .map(|tip| {
                let branch = branches.iter().find(|branch| branch.name == tip.name);
                branch.map_or(tip.name.clone(), |branch| branch.local_name.clone())
            })
            .collect();
        self.colors = ref_colors(&color_keys);
        let shas: Vec<String> = self.tips.iter().map(|tip| tip.sha.clone()).collect();
        self.commit_colors = commit_colors(&reach_masks(&self.commits, &shas), &self.colors);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Checked first, as it's the way out if the chosen branches were deleted.
        if key.code == KeyCode::Char('b') {
            return Action::OpenGraphPicker;
        }
        let Some(sha) = self.commits.get(self.selected).map(|c| c.sha.clone()) else {
            return Action::None;
        };
        match key.code {
            KeyCode::Char('y') => return Action::Copy(sha),
            KeyCode::Char('c') => return Action::CompareWith(sha),
            KeyCode::Char('i') => {
                return match git::commit_details(&sha) {
                    Ok(details) => Action::OpenDetails(details),
                    Err(error) => Action::Error(error.to_string()),
                };
            }
            _ => {}
        }

        let last = self.commits.len() - 1;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('g') => self.select_commit(0),
            KeyCode::Char('G') => self.select_commit(last),
            KeyCode::Char('n') => self.next_commit(),
            KeyCode::Char('N') => self.previous_commit(),
            KeyCode::Enter => return self.open_selection(sha),
            KeyCode::Esc => {
                self.collapse();
                return Action::None;
            }
            _ => return Action::None,
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

    /// Walks down through a commit's files, if it is expanded, then on to the
    /// next commit. Past the last row, it wraps to the top.
    fn move_down(&mut self) {
        if let Some(expanded) = self.expanded_at(self.selected) {
            let next = self.selected_file.map_or(0, |file| file + 1);
            if next < expanded.files.len() {
                self.selected_file = Some(next);
                return;
            }
        }
        self.select_commit(ui::next_index(self.selected, self.commits.len()));
    }

    /// Jumps to the next commit, past the files of an expanded one.
    fn next_commit(&mut self) {
        self.select_commit(ui::next_index(self.selected, self.commits.len()));
    }

    /// Jumps to the previous commit, past the files of an expanded one. From
    /// one of a commit's files, goes back up to the commit first, like going
    /// to the start of a hunk.
    fn previous_commit(&mut self) {
        if self.selected_file.is_some() {
            self.selected_file = None;
        } else {
            self.select_commit(ui::previous_index(self.selected, self.commits.len()));
        }
    }

    /// The mirror of `move_down`: into the files of an expanded commit from below.
    fn move_up(&mut self) {
        if let Some(file) = self.selected_file {
            self.selected_file = file.checked_sub(1);
            return;
        }
        self.select_commit(ui::previous_index(self.selected, self.commits.len()));
        self.selected_file = self
            .expanded_at(self.selected)
            .and_then(|expanded| expanded.files.len().checked_sub(1));
    }

    /// The diffs one key away among the open commits' files, most likely
    /// first: the selected file's, then the files j and k lead to. From a
    /// commit, j leads into its first file and k into the last file above.
    /// The other jumps (n/N, g/G) land on commits, which have no diff.
    pub fn file_sources_around(&self) -> Vec<DiffSource> {
        // Every file row, in the order they're shown.
        let files: Vec<(usize, usize)> = (0..self.commits.len())
            .filter_map(|commit| self.expanded_at(commit).map(|e| (commit, e.files.len())))
            .flat_map(|(commit, count)| (0..count).map(move |file| (commit, file)))
            .collect();
        let picks: Vec<(usize, usize)> = match self.selected_file {
            Some(file) => {
                let Some(here) = files.iter().position(|&row| row == (self.selected, file)) else {
                    return Vec::new();
                };
                [Some(here), Some(here + 1), here.checked_sub(1)]
                    .into_iter()
                    .flatten()
                    .filter_map(|index| files.get(index).copied())
                    .collect()
            }
            None => {
                let below = files.iter().find(|(commit, _)| *commit == self.selected);
                let above = files.iter().rev().find(|(commit, _)| commit + 1 == self.selected);
                below.into_iter().chain(above).copied().collect()
            }
        };
        picks.into_iter().filter_map(|(commit, file)| self.file_source(commit, file)).collect()
    }

    fn file_source(&self, commit: usize, file: usize) -> Option<DiffSource> {
        let expanded = self.expanded_at(commit)?;
        let path = expanded.files.get(file)?.path.clone();
        Some(DiffSource::Commit { sha: expanded.sha.clone(), path })
    }

    pub fn selected_file_source(&self) -> Option<DiffSource> {
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
        let block = ui::panel(TITLE.to_string(), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        self.render_chain(frame, inner, focused);
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
                Display::Commit(index) => self.selected_file.is_none() && *index == self.selected,
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
        let graph_width = self.rows.iter().map(|row| row.glyphs.len()).max().unwrap_or(0);

        let mut lines: Vec<Line> = display
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|row| match row {
                Display::Commit(index) => self.commit_line(*index, graph_width, focused),
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

    /// One row per commit, with each expanded commit's files inserted under it.
    fn display_rows(&self) -> Vec<Display> {
        let mut display = Vec::with_capacity(self.rows.len());
        for commit in 0..self.rows.len() {
            display.push(Display::Commit(commit));
            if let Some(expanded) = self.expanded_at(commit) {
                display.extend((0..expanded.files.len()).map(|file| Display::File { commit, file }));
            }
        }
        display
    }

    fn glyph_span(&self, glyph: &Glyph) -> Span<'static> {
        let color = glyph.commit.and_then(|i| self.commit_colors.get(i).copied());
        let color = color.unwrap_or(PALETTE[0]);
        let unpushed = glyph.commit.is_some_and(|i| self.unpushed[i]);
        let ch = if is_bead(glyph.ch) && unpushed { hollow(glyph.ch) } else { glyph.ch };
        Span::styled(ch.to_string(), Style::default().fg(color))
    }

    fn commit_line(&self, index: usize, graph_width: usize, focused: bool) -> Line<'_> {
        let glyphs = &self.rows[index].glyphs;
        let selected = index == self.selected && !self.is_on_file();
        let mut spans: Vec<Span> = glyphs.iter().map(|g| self.glyph_span(g)).collect();
        spans.push(Span::raw(" ".repeat(graph_width - glyphs.len() + 1)));

        let commit = &self.commits[index];
        let subject = if selected && focused { Color::White } else { Color::Gray };
        spans.push(Span::styled(commit.subject.as_str(), Style::default().fg(subject)));
        spans.push(Span::styled(format!("  {}", commit.author), Style::default().fg(ui::DIM)));
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
        let below = &self.rows[commit].below;

        let mut spans: Vec<Span> = below.iter().map(|g| self.glyph_span(g)).collect();
        spans.push(Span::raw(" ".repeat(graph_width.saturating_sub(below.len()) + 1)));
        let selected = commit == self.selected && self.selected_file == Some(file);
        let mut line = file_line(&expanded.files[file], selected, focused);
        spans.append(&mut line.spans);
        line.spans = spans;
        line
    }
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
                let text: String =
                    row.glyphs.iter().map(|g| if is_bead(g.ch) { '●' } else { g.ch }).collect();
                text.trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn linear_history_is_one_row_per_commit() {
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
        assert_eq!(draw(&commits), ["●", "●", "│ ●", "●─╯", "●"]);
    }

    #[test]
    fn merge_forks_then_rejoins() {
        let commits = [
            commit("m", &["p1", "p2"]),
            commit("p1", &["base"]),
            commit("p2", &["base"]),
            commit("base", &[]),
        ];
        assert_eq!(draw(&commits), ["●─╮", "● │", "│ ●", "●─╯"]);
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
        assert_eq!(draw(&commits), ["●", "├─●", "│ ●", "●─╯"]);
    }

    #[test]
    fn three_tips_join_on_one_row() {
        let commits = [
            commit("a", &["base"]),
            commit("b", &["base"]),
            commit("c", &["base"]),
            commit("base", &[]),
        ];
        assert_eq!(draw(&commits), ["●", "│ ●", "│ │ ●", "●─┴─╯"]);
    }

    #[test]
    fn connector_passing_over_a_lane_draws_a_crossing() {
        let commits = [
            commit("a", &["merge"]),
            commit("b", &["other"]),
            commit("c", &["far"]),
            commit("merge", &["first", "far"]),
        ];
        assert_eq!(draw(&commits).last().unwrap(), "●─┼─┤");
    }

    #[test]
    fn beads_reach_the_edges_their_lines_leave_through() {
        let commits = [
            commit("a", &["m"]),
            commit("m", &["p1", "p2"]),
            commit("p1", &["root"]),
            commit("p2", &["root"]),
            commit("root", &[]),
        ];
        let beads: Vec<char> = layout(&commits)
            .iter()
            .map(|row| row.glyphs.iter().find(|g| is_bead(g.ch)).unwrap().ch)
            .collect();
        assert_eq!(
            beads,
            [
                bead_char(false, true, false, false), // a tip: only down
                bead_char(true, true, false, true),   // the merge forks right
                bead_char(true, true, false, false),  // straight through
                bead_char(true, true, false, false),  // p2 in the second lane
                bead_char(true, false, false, true),  // the join comes from the right
            ]
        );
    }

    #[test]
    fn a_lone_commit_has_no_lines() {
        assert_eq!(layout(&[commit("a", &[])])[0].glyphs[0].ch, '\u{f5ee}');
    }

    #[test]
    fn a_bead_reaches_left_when_merging_into_a_lane_on_its_left() {
        let commits = [commit("tip", &["shared"]), commit("m", &["first", "shared"])];
        // m is a new tip in the second lane, so nothing comes in from above.
        assert_eq!(layout(&commits)[1].glyphs[2].ch, bead_char(false, true, true, false));
    }

    #[test]
    fn every_bead_has_a_hollow_version_next_to_it() {
        for ch in ['\u{f5ee}', '\u{f60c}'] {
            assert!(is_bead(hollow(ch)));
        }
        assert_eq!(hollow('\u{f5ee}'), '\u{f5ef}');
    }

    #[test]
    fn a_merge_does_not_reuse_the_column_of_a_line_joining_it() {
        let commits = [
            commit("a", &["m"]),
            commit("b", &["m"]),
            commit("m", &["p1", "p2"]),
            commit("p1", &[]),
            commit("p2", &[]),
        ];
        assert_eq!(draw(&commits), ["●", "│ ●", "●─┴─╮", "●   │", "    ●"]);
    }

    #[test]
    fn a_merge_can_start_a_line_inside_one_joining_it() {
        let commits = [
            commit("a", &["m"]),
            commit("x", &["z"]),
            commit("b", &["m"]),
            commit("z", &[]),
            commit("m", &["p1", "p2"]),
        ];
        assert_eq!(draw(&commits).last().unwrap(), "●─┬─╯");
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
    fn more_refs_than_fit_in_a_mask_do_not_crash() {
        let tips: Vec<String> = (0..70).map(|i| format!("tip{i}")).collect();
        let commits: Vec<Commit> = tips.iter().map(|tip| commit(tip, &[])).collect();
        let masks = reach_masks(&commits, &tips);
        let colors = commit_colors(&masks, &ref_colors(&tips));
        assert_eq!(colors.len(), 70);
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
        assert_eq!(rows[2].glyphs[2], Glyph { ch: '╯', commit: Some(2) });
        // The line passing `b` is also heading for `base`.
        assert_eq!(rows[1].glyphs[0], Glyph { ch: '│', commit: Some(2) });
    }

    #[test]
    fn overlapping_lines_show_each_color_up_to_where_it_ends() {
        let commits = [
            commit("a", &["m"]),
            commit("b", &["m"]),
            commit("m", &["p1", "p2"]),
            commit("p1", &[]),
            commit("p2", &[]),
        ];
        let glyphs = &layout(&commits)[2].glyphs;
        let leads_to: Vec<_> = glyphs[1..5].iter().map(|g| g.commit).collect();
        assert_eq!(leads_to, [Some(2), Some(2), Some(4), Some(4)]);
    }

    #[test]
    fn lines_below_a_commit_are_kept_for_its_drop_down() {
        let commits = [commit("a", &["b"]), commit("b", &[])];
        let rows = layout(&commits);
        assert_eq!(rows[0].below[0].ch, '│');
        assert_eq!(rows[1].below[0].ch, ' ');
    }

    #[test]
    fn a_merges_second_line_runs_beside_its_drop_down() {
        let commits = [commit("m", &["p1", "p2"]), commit("p1", &[]), commit("p2", &[])];
        let rows = layout(&commits);
        let below: String = rows[0].below.iter().map(|g| g.ch).collect();
        assert_eq!(below.trim_end(), "│ │");
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
    fn n_and_shift_n_skip_over_the_files_of_open_commits() {
        let mut panel = panel_with_expanded(&["a", "b", "c"], &["a", "b"], &["x", "y"]);
        panel.next_commit();
        assert_eq!((panel.selected, panel.selected_file), (1, None));
        panel.next_commit();
        assert_eq!((panel.selected, panel.selected_file), (2, None));
        panel.previous_commit();
        assert_eq!((panel.selected, panel.selected_file), (1, None));
    }

    #[test]
    fn shift_n_on_a_file_goes_back_to_its_commit() {
        let mut panel = panel_with_expanded(&["a", "b"], &["b"], &["x", "y"]);
        panel.selected = 1;
        panel.selected_file = Some(1);
        panel.previous_commit();
        assert_eq!((panel.selected, panel.selected_file), (1, None));
        panel.previous_commit();
        assert_eq!((panel.selected, panel.selected_file), (0, None));
    }

    #[test]
    fn n_on_a_file_goes_to_the_next_commit() {
        let mut panel = panel_with_expanded(&["a", "b"], &["a"], &["x", "y"]);
        panel.selected_file = Some(0);
        panel.next_commit();
        assert_eq!((panel.selected, panel.selected_file), (1, None));
    }

    fn paths(sources: Vec<DiffSource>) -> Vec<String> {
        sources.iter().map(|source| format!("{}:{}", match source {
            DiffSource::Commit { sha, .. } => sha.as_str(),
            _ => "?",
        }, source.path())).collect()
    }

    #[test]
    fn the_files_around_a_file_are_its_neighbours_across_commits() {
        let mut panel = panel_with_expanded(&["a", "b"], &["a", "b"], &["x", "y"]);
        panel.selected_file = Some(1);
        assert_eq!(paths(panel.file_sources_around()), ["a:y", "b:x", "a:x"]);
    }

    #[test]
    fn the_files_around_a_commit_are_its_first_and_the_last_above() {
        let mut panel = panel_with_expanded(&["a", "b", "c"], &["a", "b"], &["x", "y"]);
        panel.selected = 1;
        assert_eq!(paths(panel.file_sources_around()), ["b:x", "a:y"]);
        panel.selected = 2;
        assert_eq!(paths(panel.file_sources_around()), ["b:y"]);
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
    fn moving_past_the_last_row_wraps_to_the_top() {
        let mut panel = panel_with_expanded(&["a", "b"], &["b"], &["x"]);
        panel.selected = 1;
        panel.selected_file = Some(0);
        panel.move_down();
        assert_eq!((panel.selected, panel.selected_file), (0, None));
    }

    #[test]
    fn moving_above_the_top_wraps_to_the_last_row() {
        let mut panel = panel_with_expanded(&["a", "b"], &[], &[]);
        panel.move_up();
        assert_eq!((panel.selected, panel.selected_file), (1, None));
    }

    #[test]
    fn moving_above_the_top_enters_an_expanded_last_commit_from_below() {
        let mut panel = panel_with_expanded(&["a", "b"], &["b"], &["x", "y"]);
        panel.move_up();
        assert_eq!((panel.selected, panel.selected_file), (1, Some(1)));
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
