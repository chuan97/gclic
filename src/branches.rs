//! Feature 3: the pane listing branches, for checking one out.

use crate::app::Action;
use crate::git::{self, Branch};
use crate::picker::{Outcome, Picker};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// A remote's copy of a local branch, for the drop-down.
struct RemoteRow {
    name: String,
    ahead: usize,
    behind: usize,
    is_upstream: bool,
}

/// A local branch opened to show its remotes.
struct DropDown {
    branch: String,
    remotes: Vec<RemoteRow>,
}

pub struct BranchesPanel {
    picker: Picker,
    branches: Vec<Branch>,
    /// The checked-out branch. None on a detached HEAD.
    current: Option<String>,
    drop_downs: Vec<DropDown>,
    scroll: usize,
}

/// The remote branches belonging to a local one: those with its name, plus
/// whatever it tracks, which can have a different name.
fn remotes_of<'a>(branches: &'a [Branch], local: &Branch) -> Vec<&'a Branch> {
    let tracks = |branch: &Branch| local.upstream.as_deref() == Some(branch.name.as_str());
    branches
        .iter()
        .filter(|branch| branch.is_remote && (branch.local_name == local.name || tracks(branch)))
        .collect()
}

/// The names to list: every local branch, then remote branches not already
/// covered by a local one, since selecting those would only switch to it.
fn listed_names(branches: &[Branch]) -> Vec<String> {
    let locals: Vec<&Branch> = branches.iter().filter(|branch| !branch.is_remote).collect();
    let covered = |remote: &Branch| {
        locals.iter().any(|local| {
            local.name == remote.local_name || local.upstream.as_deref() == Some(remote.name.as_str())
        })
    };
    branches
        .iter()
        .filter(|branch| !branch.is_remote || !covered(branch))
        .map(|branch| branch.name.clone())
        .collect()
}

impl BranchesPanel {
    pub fn new() -> BranchesPanel {
        BranchesPanel {
            picker: Picker::single("", Vec::new()),
            branches: Vec::new(),
            current: None,
            drop_downs: Vec::new(),
            scroll: 0,
        }
    }

    pub fn is_typing(&self) -> bool {
        self.picker.is_typing()
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.branches = git::branches()?;
        self.picker.set_items(listed_names(&self.branches));

        // The cursor goes to the new branch, however the switch happened.
        let current = git::current_branch();
        if current != self.current {
            if let Some(name) = &current {
                self.picker.select_item(name);
            }
            self.current = current;
        }

        // Open drop-downs stay open, with up-to-date counts.
        let open: Vec<String> = self.drop_downs.iter().map(|d| d.branch.clone()).collect();
        self.drop_downs = open.iter().filter_map(|name| self.load_drop_down(name)).collect();
        Ok(())
    }

    fn find(&self, name: &str) -> Option<&Branch> {
        self.branches.iter().find(|branch| branch.name == name)
    }

    /// None if `name` is no longer a local branch with remotes.
    fn load_drop_down(&self, name: &str) -> Option<DropDown> {
        let local = self.find(name).filter(|branch| !branch.is_remote)?;
        let remotes: Vec<RemoteRow> = remotes_of(&self.branches, local)
            .into_iter()
            .map(|remote| {
                let (ahead, behind) = git::ahead_behind(name, &remote.name).unwrap_or((0, 0));
                let is_upstream = local.upstream.as_deref() == Some(remote.name.as_str());
                RemoteRow { name: remote.name.clone(), ahead, behind, is_upstream }
            })
            .collect();
        (!remotes.is_empty()).then(|| DropDown { branch: name.to_string(), remotes })
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Char(' ') && !self.picker.is_typing() {
            return self.toggle_drop_down();
        }
        match self.picker.handle_key(key) {
            Outcome::Open => Action::None,
            Outcome::Cancelled => {
                self.picker.clear_query();
                Action::None
            }
            Outcome::Picked(picked) => {
                let name = picked.first().cloned().unwrap_or_default();
                self.check_out(&name)
            }
        }
    }

    fn toggle_drop_down(&mut self) -> Action {
        let Some(name) = self.picker.highlighted().cloned() else {
            return Action::None;
        };
        if self.drop_downs.iter().any(|drop_down| drop_down.branch == name) {
            self.drop_downs.retain(|drop_down| drop_down.branch != name);
            return Action::None;
        }
        if self.find(&name).is_none_or(|branch| branch.is_remote) {
            return Action::Message("This branch is remote only".to_string());
        }
        match self.load_drop_down(&name) {
            Some(drop_down) => {
                self.drop_downs.push(drop_down);
                Action::None
            }
            None => Action::Message("No remote has this branch".to_string()),
        }
    }

    fn check_out(&mut self, name: &str) -> Action {
        self.picker.stop_typing();
        let is_remote = self.find(name).is_some_and(|branch| branch.is_remote);
        let result = if is_remote { git::switch_track(name) } else { git::switch(name) };
        if result.is_ok() {
            self.picker.clear_query();
            self.picker.select_item(name);
        }
        Action::after_change(result)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = ui::panel(" Branches ".to_string(), focused);
        let mut list = block.inner(area);
        frame.render_widget(block, area);

        // The search line only takes up room while it's in use.
        if self.picker.is_searching() && list.height > 0 {
            self.picker.render_query(frame, Rect { height: 1, ..list });
            list.y += 1;
            list.height -= 1;
        }

        let (lines, selected_line) = self.list_lines(focused);
        if lines.is_empty() {
            let note = if self.branches.is_empty() { "No branches yet" } else { "No match" };
            frame.render_widget(Line::styled(note, Style::default().fg(ui::DIM)), list);
            return;
        }
        self.scroll = ui::keep_visible(self.scroll, selected_line, list.height as usize);
        let lines = lines.into_iter().skip(self.scroll).collect();
        ui::render_lines(frame, list, lines);
    }

    /// Every row of the list, and which of them holds the cursor.
    fn list_lines(&self, focused: bool) -> (Vec<Line<'static>>, usize) {
        let mut lines = Vec::new();
        let mut selected_line = 0;
        for (row, name) in self.picker.matched_items().into_iter().enumerate() {
            let selected = row == self.picker.selected_row();
            if selected {
                selected_line = lines.len();
            }
            lines.push(self.branch_line(name, selected && focused));
            let drop_down = self.drop_downs.iter().find(|d| d.branch == name);
            if let Some(drop_down) = drop_down {
                lines.extend(drop_down.remotes.iter().map(remote_line));
            }
        }
        (lines, selected_line)
    }

    fn branch_line(&self, name: &str, selected: bool) -> Line<'static> {
        let is_current = self.current.as_deref() == Some(name);
        let branch = self.find(name);
        let is_remote = branch.is_some_and(|branch| branch.is_remote);
        let style = if is_current {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else if is_remote {
            Style::default().fg(ui::DIM)
        } else {
            Style::default()
        };

        let mark = if is_current { "* " } else { "  " };
        let mut spans = vec![Span::styled(format!("{mark}{name}"), style)];
        let count = match branch {
            Some(local) if !local.is_remote => remotes_of(&self.branches, local).len(),
            _ => 0,
        };
        if count > 0 {
            let noun = if count == 1 { "remote" } else { "remotes" };
            spans.push(Span::styled(format!("  {count} {noun}"), Style::default().fg(ui::DIM)));
        }

        let line = Line::from(spans);
        if selected {
            line.style(Style::default().bg(ui::SELECTED_BG))
        } else {
            line
        }
    }
}

fn remote_line(remote: &RemoteRow) -> Line<'static> {
    let dim = Style::default().fg(ui::DIM);
    let mut spans = vec![Span::styled(format!("    {}  ", remote.name), dim)];
    if remote.ahead == 0 && remote.behind == 0 {
        spans.push(Span::styled("in sync", dim));
    } else {
        spans.push(Span::styled(format!("↑{}", remote.ahead), Style::default().fg(Color::Green)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("↓{}", remote.behind), Style::default().fg(Color::Red)));
    }
    if remote.is_upstream {
        spans.push(Span::styled("  tracked", dim));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str, upstream: Option<&str>) -> Branch {
        Branch {
            name: name.to_string(),
            local_name: name.to_string(),
            is_remote: false,
            upstream: upstream.map(str::to_string),
        }
    }

    fn remote(name: &str) -> Branch {
        Branch {
            name: name.to_string(),
            local_name: name.split_once('/').unwrap().1.to_string(),
            is_remote: true,
            upstream: None,
        }
    }

    #[test]
    fn remotes_with_a_local_branch_are_not_listed() {
        let branches = [local("main", None), remote("origin/main"), remote("origin/new")];
        assert_eq!(listed_names(&branches), ["main", "origin/new"]);
    }

    #[test]
    fn a_tracked_remote_with_another_name_is_not_listed() {
        let branches = [local("mine", Some("origin/theirs")), remote("origin/theirs")];
        assert_eq!(listed_names(&branches), ["mine"]);
    }

    #[test]
    fn the_same_remote_only_branch_is_listed_per_remote() {
        let branches = [remote("origin/new"), remote("upstream/new")];
        assert_eq!(listed_names(&branches), ["origin/new", "upstream/new"]);
    }

    #[test]
    fn a_local_branch_has_every_remote_with_its_name() {
        let main = local("main", Some("origin/main"));
        let branches = [
            main.clone(),
            remote("origin/main"),
            remote("upstream/main"),
            remote("origin/other"),
        ];
        let names: Vec<&str> = remotes_of(&branches, &main).iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["origin/main", "upstream/main"]);
    }

    #[test]
    fn a_local_branch_has_its_upstream_even_with_another_name() {
        let mine = local("mine", Some("origin/theirs"));
        let branches = [mine.clone(), remote("origin/theirs")];
        assert_eq!(remotes_of(&branches, &mine).len(), 1);
    }

    #[test]
    fn a_nested_branch_name_matches_its_remote() {
        let nested = local("a/b", None);
        let branches = [nested.clone(), remote("origin/a/b")];
        assert_eq!(remotes_of(&branches, &nested).len(), 1);
    }
}
