//! Drawing the screen: the colour bar, the tabs, the panes on the left and the
//! preview.

use super::*;

impl App {
    pub fn render(&mut self, frame: &mut Frame) {
        // In Neovim's sidebar, the editor's own bar shows the repo's colour.
        let bar_shown = self.accent_bar && self.sidebar.is_none();
        let [bar, tabs, main, status] = Layout::vertical([
            Constraint::Length(u16::from(bar_shown)),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.render_accent_bar(frame, bar);
        self.render_tabs(frame, tabs);

        let sidebar = self.sidebar_column(main);
        let waiting = self.waiting_for_room(main);
        match (self.view.has_preview(), self.view.zoomed, waiting) {
            (false, _, _) => self.render_left(frame, sidebar.unwrap_or(main)),
            (true, true, _) => self.render_preview(frame, main),
            (true, false, true) => self.render_left(frame, main),
            // Too narrow for both side by side: the diff or details on their
            // own, and esc goes back to the panes.
            (true, false, false) if main.width < NARROW_BELOW => self.render_preview(frame, main),
            (true, false, false) => {
                // The review's comments are read in the list, so it gets more room.
                let diff_percent = if self.tab == Tab::Review {
                    REVIEW_DIFF_PERCENT
                } else {
                    DIFF_PERCENT
                };
                let panes = match self.panes_width {
                    // Set, as Neovim's plugin does: never more than half,
                    // leaving the diff room.
                    Some(width) if self.tab == Tab::Repo => {
                        Constraint::Length(width.min(main.width / 2))
                    }
                    _ => Constraint::Percentage(100 - diff_percent),
                };
                let [left, right] = Layout::horizontal([panes, Constraint::Min(0)]).areas(main);
                self.render_left(frame, left);
                self.render_preview(frame, right);
            }
        }
        self.render_status_line(frame, status);
        self.render_modal(frame);
    }

    /// The repo's colour, as a line across the top ending in the repo's
    /// folder and name, as Neovim's bar shows them with gclic's plugin.
    pub(super) fn render_accent_bar(&self, frame: &mut Frame, area: Rect) {
        let (folder, name) = &self.project;
        let (line, label) = accent_bar(area.width as usize, folder, name);
        let color = Style::default().fg(self.accent.color());
        let mut spans = vec![Span::raw(" "), Span::styled(line, color)];
        if let Some((folder, name)) = label {
            spans.push(Span::styled(
                format!(" {folder}"),
                Style::default().fg(ui::DIM),
            ));
            spans.push(Span::styled(name, color.add_modifier(Modifier::BOLD)));
        }
        spans.push(Span::raw(" "));
        frame.render_widget(Line::from(spans), area);
    }

    /// The tab bar: the tabs, the showing one highlighted, the comparison's
    /// refs on its tab so it's visible from the Repo tab too, and the review's
    /// count on its own. The tab `tab` leads to is marked with that key, like
    /// the panes' [h] and [l]. The
    /// project goes on the right: its folder dimmed, and its name in the repo's
    /// colour, shortened to fit.
    pub(super) fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let area = Rect {
            x: area.x + 1,
            width: area.width.saturating_sub(2),
            ..area
        };
        let compare = match &self.compare {
            Some(compare) => format!(" {} ", compare.title()),
            None => " Compare ".to_string(),
        };
        let next = self.next_tab(true);
        let mut spans = Vec::new();
        for tab in self.tabs() {
            let label = match tab {
                Tab::Repo => " Repo ".to_string(),
                Tab::Compare => compare.clone(),
                Tab::Review => format!(" {} ({}) ", review::TITLE, self.review.len()),
            };
            if tab == self.tab {
                let style = Style::default()
                    .fg(Color::Cyan)
                    .bg(ui::SELECTED_BG)
                    .add_modifier(Modifier::BOLD);
                spans.push(Span::styled(label, style));
            } else {
                spans.push(Span::styled(label, Style::default().fg(ui::DIM)));
                if tab == next {
                    spans.push(Span::styled("[tab]", Style::default().fg(Color::Gray)));
                }
            }
            spans.push(Span::raw("  "));
        }
        let tabs = Line::from(spans);
        // The colour bar shows the project when it's on.
        if self.accent_bar {
            frame.render_widget(tabs, area);
            return;
        }
        let name_color = Color::Cyan;
        // Two columns at least between the tabs and the project.
        let room = (area.width as usize).saturating_sub(tabs.width() + 2);
        let (folder, name) = &self.project;
        if let Some(folder) = fit_folder(folder, name, room) {
            let project = Line::from(vec![
                Span::styled(folder, Style::default().fg(ui::DIM)),
                Span::styled(
                    name.as_str(),
                    Style::default().fg(name_color).add_modifier(Modifier::BOLD),
                ),
            ]);
            frame.render_widget(project.right_aligned(), area);
        }
        frame.render_widget(tabs, area);
    }

    pub(super) fn render_left(&mut self, frame: &mut Frame, area: Rect) {
        if self.tab == Tab::Review {
            self.review_list
                .render(frame, area, self.view.focused(Pane::Review), &self.review);
            return;
        }
        if self.tab == Tab::Compare {
            let focused = self.view.focused(Pane::Compare);
            match &mut self.compare {
                Some(compare) => compare.render(frame, area, focused),
                None => render_empty_compare(frame, area, focused),
            }
            return;
        }
        // Branches and Changes take the rows they ask for, up to a share of the
        // height, and the graph gets the rest. Borders add two rows to each.
        let fit =
            |rows: u16, max_percent: u16| (rows + 2).min(area.height * max_percent / 100).max(3);
        let branches_height = fit(
            self.branches
                .wanted_height(self.view.focused(Pane::Branches)),
            BRANCHES_MAX_PERCENT,
        );
        let changes_max = if self.view.focused(Pane::Changes) {
            CHANGES_FOCUSED_MAX_PERCENT
        } else {
            CHANGES_MAX_PERCENT
        };
        let changes_height = fit(self.staging.wanted_height(), changes_max);
        let [branches, changes, graph] = Layout::vertical([
            Constraint::Length(branches_height),
            Constraint::Length(changes_height),
            Constraint::Min(0),
        ])
        .areas(area);
        let note = self.fetcher.note();
        self.branches.render(
            frame,
            branches,
            self.view.focused(Pane::Branches),
            note.as_deref(),
        );
        self.staging
            .render(frame, changes, self.view.focused(Pane::Changes));
        self.graph
            .render(frame, graph, self.view.focused(Pane::Graph));

        // The panes that h and l lead to say so next to their title, in place
        // of a hint. With three panes and wrapping, those are the two not focused.
        let changes_title = self.staging.title();
        let panes = [
            (Pane::Branches, branches, crate::branches::TITLE),
            (Pane::Changes, changes, changes_title.as_str()),
            (Pane::Graph, graph, crate::graph::TITLE),
        ];
        if let Some(current) = panes.iter().position(|(pane, ..)| self.view.focused(*pane)) {
            let (_, previous, title) = panes[(current + 2) % 3];
            mark_key(frame, previous, title, "h");
            let (_, next, title) = panes[(current + 1) % 3];
            mark_key(frame, next, title, "l");
        }
    }

    pub(super) fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.view.in_preview;
        match &mut self.view.preview {
            Preview::Diff(diff) => {
                diff.set_comments(self.review.on_diff(&diff.source));
                diff.render(frame, area, focused, self.split);
            }
            Preview::Details(details) => details.render(frame, area, focused),
            Preview::None => {}
        }
    }
}

/// Writes `[key]` on a pane's top border, right after its title.
pub(super) fn mark_key(frame: &mut Frame, pane: Rect, title: &str, key: &str) {
    // The trailing space keeps it apart from the border line, like the title.
    let label = format!("[{key}] ");
    // One column for the corner, and one more to leave before the other corner.
    let start = 1 + title.chars().count() as u16;
    let width = label.len() as u16;
    if pane.height == 0 || start + width + 1 > pane.width {
        return;
    }
    let area = Rect {
        x: pane.x + start,
        y: pane.y,
        width,
        height: 1,
    };
    frame.render_widget(Line::styled(label, Style::default().fg(Color::Gray)), area);
}

/// The Compare tab before a comparison: how to start one.
pub(super) fn render_empty_compare(frame: &mut Frame, area: Rect, focused: bool) {
    let block = ui::panel(" Compare ".to_string(), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let line = Line::from(vec![
        Span::raw("No comparison yet. "),
        Span::styled("c", Style::default().fg(ui::KEY)),
        Span::raw(" starts one."),
    ]);
    frame.render_widget(line, inner);
}

/// The colour bar in `width` columns: its line, and the folder and name after
/// it, the folder shortened or left out where there's no room for it, and
/// the name too, so that at least a short line shows. A column of margin at
/// each end, as the panes have.
/// What the colour bar's line is drawn with: the middle third of a cell,
/// level with the name after it, and thicker than a line character, whose
/// thickness is up to the font. Most terminals draw it themselves, to fill
/// the cell exactly.
pub(super) const ACCENT_LINE: &str = "🬋";

pub(super) fn accent_bar(
    width: usize,
    folder: &str,
    name: &str,
) -> (String, Option<(String, String)>) {
    const SHORTEST_LINE: usize = 4;
    let inside = width.saturating_sub(2);
    // A space between the line and the folder.
    let room = inside.saturating_sub(SHORTEST_LINE + 1);
    let label = fit_folder(folder, name, room).map(|folder| (folder, name.to_string()));
    let label_width = label.as_ref().map_or(0, |(folder, name)| {
        1 + folder.chars().count() + name.chars().count()
    });
    (ACCENT_LINE.repeat(inside - label_width), label)
}

/// The folder holding the repo, ending in a separator and with `~` for the
/// home folder, and the repo's own folder name.
pub(super) fn project_location(root: &Path) -> (String, String) {
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folder = root.parent().map(repos::tilde).unwrap_or_default();
    let separator = if folder.ends_with(std::path::MAIN_SEPARATOR) {
        ""
    } else {
        std::path::MAIN_SEPARATOR_STR
    };
    (format!("{folder}{separator}"), name)
}

/// The folder to show before the name in `room` columns: all of it, its end
/// after an ellipsis, or nothing. None if even the name doesn't fit.
pub(super) fn fit_folder(folder: &str, name: &str, room: usize) -> Option<String> {
    let name_width = name.chars().count();
    if name_width > room {
        return None;
    }
    let left = room - name_width;
    let width = folder.chars().count();
    if width <= left {
        return Some(folder.to_string());
    }
    // Room for the ellipsis and at least a few characters, or it says nothing.
    if left < 6 {
        return Some(String::new());
    }
    let tail: String = folder.chars().skip(width - (left - 1)).collect();
    Some(format!("…{tail}"))
}
