//! Feature 3: the pane listing branches, for checking one out.

use crate::app::Action;
use crate::git::{self, Branch};
use crate::picker::{Outcome, Picker};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

pub struct BranchesPanel {
    picker: Picker,
    branches: Vec<Branch>,
    /// The checked-out branch. None on a detached HEAD.
    current: Option<String>,
}

impl BranchesPanel {
    pub fn new() -> BranchesPanel {
        BranchesPanel { picker: Picker::pane(), branches: Vec::new(), current: None }
    }

    pub fn is_typing(&self) -> bool {
        self.picker.is_typing()
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.branches = git::branches()?;
        self.picker.set_items(self.branches.iter().map(|b| b.name.clone()).collect());

        // The cursor goes to the new branch, however the switch happened.
        let current = git::current_branch();
        if current != self.current {
            if let Some(name) = &current {
                self.picker.select_item(name);
            }
            self.picker.set_current(current.clone());
            self.current = current;
        }
        Ok(())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
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

    fn check_out(&mut self, name: &str) -> Action {
        self.picker.stop_typing();
        let branch = self.branches.iter().find(|branch| branch.name == name);
        let local_name = branch.map_or(name, |branch| branch.local_name.as_str());
        let result = git::switch(local_name);
        if result.is_ok() {
            self.picker.clear_query();
            self.picker.select_item(name);
        }
        Action::after_change(result)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = ui::panel(" Branches ".to_string(), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.picker.render_list(frame, inner, focused);
    }
}
