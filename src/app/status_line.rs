//! The line at the bottom: the keys that apply where the user is.

use super::*;

impl App {
    /// The pane's own keys, then the keys that work everywhere.
    pub(super) fn render_status_line(&self, frame: &mut Frame, area: Rect) {
        // A column of margin at each end, which also lines the text up with
        // the text inside the panes' borders.
        let area = Rect { x: area.x + 1, width: area.width.saturating_sub(2), ..area };
        // The command line takes the place of the hints, as in vim.
        if let Modal::Command(line) = &self.modal {
            let cyan = Style::default().fg(Color::Cyan);
            let dim = Style::default().fg(ui::DIM);
            let mut spans = vec![Span::styled(":", cyan), Span::raw(line.as_str()), Span::styled("▏", cyan)];
            // The rest of a suggested command, after what's typed, as fish
            // shows its suggestions.
            let rest = self.suggestion.as_deref().and_then(|suggestion| suggestion.strip_prefix(line.as_str()));
            if let Some(rest) = rest.filter(|rest| !rest.is_empty()) {
                spans.push(Span::styled(rest, dim));
                spans.push(Span::styled("   tab use it · esc cancel", dim));
            } else {
                // What the commands are, or the one being typed takes, in the
                // room left after the cursor.
                let used = 2 + line.chars().count();
                let (ghost, hint) = commands::command_hint(line, (area.width as usize).saturating_sub(used + 3));
                if !hint.is_empty() || !ghost.is_empty() {
                    spans.push(Span::styled(ghost, dim));
                    spans.push(Span::styled(format!("  {hint}"), dim));
                }
            }
            frame.render_widget(Line::from(spans), area);
            return;
        }
        // A popup takes every key and lists its own on its border.
        if !matches!(self.modal, Modal::None) {
            return;
        }
        let dim = Style::default().fg(ui::DIM);
        // After g, what can follow it.
        if self.pending_g {
            let mut hints = vec![("gg", "top")];
            hints.extend(self.comment_hints());
            hints.push(("esc", "cancel"));
            frame.render_widget(Line::from(hint_spans(&hints)), area);
            return;
        }
        let (mut spans, empty) = if let Some(running) = self.running.as_ref().filter(|_| self.message.is_empty()) {
            let running = format!("running {}… ", running.label);
            (vec![Span::styled(running, Style::default().fg(Color::Yellow))]
                .into_iter()
                .chain(hint_spans(&[("ctrl-c", "stop")]))
                .collect(), false)
        } else if self.message.is_empty() {
            let hints = self.hints();
            (hint_spans(&hints), hints.is_empty())
        } else {
            (vec![Span::styled(self.message.clone(), Style::default().fg(Color::Yellow))], false)
        };
        // The keys that work everywhere follow, set apart from the pane's own.
        // While typing a branch search, ? and q are typed into it instead.
        if !(self.view.focused(Pane::Branches) && self.branches.is_typing()) {
            if !empty {
                spans.push(Span::styled("   │   ", dim));
            }
            spans.extend(hint_spans(&GLOBAL_HINTS));
        }
        frame.render_widget(Line::from(spans), area);
    }

    /// The keys that do something right now where the user is, so the line
    /// only lists what applies: the focused pane's own, then those of the
    /// diff or details on the right, then o and esc.
    pub(super) fn hints(&self) -> Vec<ui::Hint<'_>> {
        let shown = self.view.diff().map(|diff| &diff.source);
        let mut hints: Vec<ui::Hint> = Vec::new();
        // What esc does, unless an open diff or details take it.
        let mut esc = None;
        if self.view.in_preview {
            if let Some(diff) = self.view.diff() {
                // On a change of the working tree, it can be staged alone.
                if diff.active_change().is_some() {
                    match diff.source {
                        DiffSource::Unstaged(_) | DiffSource::Untracked(_) => {
                            hints.extend([("space", "stage change"), ("d", "discard change")]);
                        }
                        DiffSource::Staged(_) => hints.push(("space", "unstage change")),
                        _ => {}
                    }
                }
                hints.extend(self.comment_hints());
                hints.extend(self.diff_view_hints());
            } else {
                hints.push(("y", "copy sha"));
            }
            hints.push(if self.view.zoomed { ("z", "unzoom") } else { ("z", "zoom") });
            esc = Some("back");
        } else {
            match self.view.pane {
                // Every other key is typed into the search.
                Pane::Branches if self.branches.is_typing() => return self.branches.hints(),
                Pane::Branches => {
                    hints.extend(self.branches.hints());
                    esc = self.branches.esc_hint();
                }
                Pane::Changes => hints.extend(self.staging.hints(shown)),
                Pane::Graph => {
                    let details = self.view.details().map(|details| details.sha());
                    hints.extend(self.graph.hints(shown, details));
                    esc = self.graph.esc_hint();
                }
                Pane::Compare => match &self.compare {
                    Some(compare) => {
                        hints.extend(compare.hints(shown));
                        esc = Some(compare.esc_hint());
                    }
                    None => {
                        hints.push(("c", "choose two refs"));
                        esc = Some("back to Repo");
                    }
                },
                Pane::Review => hints.extend(self.review_list.hints()),
            }
            hints.extend(self.comment_hints());
            hints.extend(self.diff_view_hints());
        }
        if self.focused_file().is_some() {
            hints.push(("o", self.editor_hint.as_str()));
        }
        if self.where_staging_is() && self.history.can_undo() {
            hints.push(("u", "undo"));
        }
        if self.where_staging_is() && self.history.can_redo() {
            hints.push(("ctrl-r", "redo"));
        }
        if !self.view.in_preview {
            if self.view.diff().is_some() {
                esc = Some("close diff");
            } else if self.view.details().is_some() {
                esc = Some("close details");
            }
        }
        hints.extend(esc.map(|esc| ("esc", esc)));
        hints
    }

    /// The file under the cursor: the focused diff's, or the one selected in
    /// the focused pane.
    pub(super) fn focused_file(&self) -> Option<DiffSource> {
        if self.view.in_preview {
            return self.view.diff().map(|diff| diff.source.clone());
        }
        match self.view.pane {
            Pane::Branches => None,
            Pane::Changes => self.staging.selected_source(),
            Pane::Graph => self.graph.selected_file_source(),
            Pane::Compare => self.compare.as_ref().and_then(|compare| compare.selected_source()),
            Pane::Review => {
                let comment = self.review_list.selected(&self.review).and_then(|index| self.review.get(index));
                comment.map(|comment| self.review_source(comment))
            }
        }
    }

    /// The help section for where the user is, which help marks.
    pub(super) fn help_section(&self) -> &'static str {
        if self.view.in_preview {
            return if self.view.diff().is_some() { "Diff" } else { "Commit details" };
        }
        match self.view.pane {
            Pane::Branches => "Branches",
            Pane::Changes => "Changes",
            Pane::Graph => "Graph",
            Pane::Compare => "Compare",
            Pane::Review => "Review",
        }
    }

    /// gc and gC, where they'd comment: on the diff's active line when it
    /// shows, and on its file whenever there's one to comment on.
    pub(super) fn comment_hints(&self) -> Vec<ui::Hint<'static>> {
        let Some(diff) = self.view.diff() else {
            return Vec::new();
        };
        if review::Place::of(&diff.source).is_none() {
            return Vec::new();
        }
        let mut hints = Vec::new();
        let on_line = diff.shows_active_line(self.view.in_preview) && diff.active_anchor().is_some();
        if on_line {
            hints.push(if diff.comment_at_active().is_some() { ("gc", "edit comment") } else { ("gc", "comment") });
        }
        let on_file = self.review.find(&diff.source, None).is_some();
        hints.push(if on_file { ("gC", "edit file comment") } else { ("gC", "comment file") });
        hints
    }

    /// The keys that work on a diff on screen, from any pane.
    pub(super) fn diff_view_hints(&self) -> Vec<ui::Hint<'static>> {
        let Some(diff) = self.view.diff() else {
            return Vec::new();
        };
        vec![
            ("n/N", "change"),
            if self.whole { ("e", "changes only") } else { ("e", "whole file") },
            if diff.is_split() { ("s", "unified") } else { ("s", "side by side") },
        ]
    }

}

/// Hints as popups show keys, `space stage · enter diff`: each key in the key
/// colour, and what it does in the normal text colour.
pub(super) fn hint_spans(hints: &[ui::Hint]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (key, action)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(ui::DIM)));
        }
        if key.is_empty() {
            spans.push(Span::raw(action.to_string()));
        } else {
            spans.push(Span::styled(key.to_string(), Style::default().fg(ui::KEY)));
            spans.push(Span::raw(format!(" {action}")));
        }
    }
    spans
}
