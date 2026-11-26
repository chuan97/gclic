//! gclic in a sidebar beside the code, through its Neovim plugin. gclic says
//! where it should show: over the whole editor while there's a right side to
//! show, a diff or details, or another tab than Repo, and in the sidebar
//! otherwise. The plugin moves the one gclic between the two, so everything
//! stays as it was, and the Repo tab's panes keep the sidebar's width in
//! both, so only the right side comes and goes.

use super::*;

impl App {
    /// Whether gclic has more to show than the sidebar's panes.
    pub(super) fn wants_full(&self) -> bool {
        self.view.has_preview() || self.tab != Tab::Repo
    }

    /// Where gclic should show now, if that's changed since it last asked:
    /// true for the whole editor. Only in Neovim's sidebar.
    pub(super) fn layout_change(&mut self) -> Option<bool> {
        self.sidebar?;
        let full = self.wants_full();
        (full != self.asked_full).then(|| {
            self.asked_full = full;
            full
        })
    }

    /// Asks the plugin to move gclic, if it should move.
    pub(super) fn ask_for_layout(&mut self) {
        if let Some(full) = self.layout_change()
            && let Some(address) = editor::parent_nvim()
        {
            self.start_in_background("nvim", editor::layout_nvim(&address, full));
        }
    }

    /// Back to the sidebar: the Repo tab, without its right side.
    pub(super) fn back_to_sidebar(&mut self) {
        self.go_to_tab(Tab::Repo);
        self.view.zoomed = false;
        self.view.in_preview = false;
        self.close_preview();
    }

    /// ctrl-g: back to the sidebar from the whole editor, or hidden from the
    /// sidebar.
    pub(super) fn shrink(&mut self) {
        if self.wants_full() {
            self.back_to_sidebar();
        } else if let Some(address) = editor::parent_nvim() {
            self.start_in_background("nvim", editor::hide_nvim(&address));
        }
    }

    /// The Repo tab's panes in the sidebar's width, from the left of `area`,
    /// in Neovim's sidebar. None elsewhere.
    pub(super) fn sidebar_column(&self, area: Rect) -> Option<Rect> {
        let width = self.sidebar?;
        (self.tab == Tab::Repo).then(|| Rect {
            width: width.min(area.width),
            ..area
        })
    }
}
