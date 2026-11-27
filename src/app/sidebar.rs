//! gclic in a sidebar beside the code, through its Neovim plugin. gclic says
//! where it should show: over the whole editor while there's a diff or
//! details to show, another tab than Repo, or after z, and in the sidebar
//! otherwise. The plugin moves the one gclic between the two, so everything
//! stays as it was. Over the whole editor, gclic is laid out as it is
//! anywhere else.

use super::*;

impl App {
    /// Whether gclic should show over the whole editor.
    pub(super) fn wants_full(&self) -> bool {
        self.chose_full || self.view.has_preview() || self.tab != Tab::Repo
    }

    /// z without a diff or details: over the whole editor from the sidebar,
    /// and back from there, the biggest gclic is without one.
    pub(super) fn grow_or_shrink(&mut self) {
        if self.wants_full() {
            self.back_to_sidebar();
        } else {
            self.chose_full = true;
        }
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
        self.chose_full = false;
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

    /// Where the panes go while gclic should be in the sidebar, but may still
    /// be over the whole editor, before the plugin has moved it back: in the
    /// sidebar's width, so they don't jump wide for a moment. None otherwise.
    pub(super) fn sidebar_column(&self, area: Rect) -> Option<Rect> {
        let width = self.sidebar?;
        (!self.wants_full()).then(|| Rect {
            width: width.min(area.width),
            ..area
        })
    }

    /// Whether gclic, in the sidebar, is still in its width, with a diff or
    /// details to show once the plugin has moved it over the whole editor:
    /// the panes alone until then, rather than the diff squeezed in.
    pub(super) fn waiting_for_room(&self, area: Rect) -> bool {
        self.sidebar.is_some_and(|width| area.width <= width)
    }
}
