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
                let (ghost, hint) = crate::commands::command_hint(line, (area.width as usize).saturating_sub(used + 3));
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
            let mut hints = vec!["gg top"];
            hints.extend(self.comment_hints());
            hints.push("esc cancel");
            let spans = hint_spans(&hints.join(" · "));
            frame.render_widget(Line::from(spans), area);
            return;
        }
        let (mut spans, empty) = if let Some(running) = self.running.as_ref().filter(|_| self.message.is_empty()) {
            let running = format!("running {}… ", running.label);
            (vec![Span::styled(running, Style::default().fg(Color::Yellow))]
                .into_iter()
                .chain(hint_spans("ctrl-c stop"))
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
            spans.extend(hint_spans(GLOBAL_HINTS));
        }
        frame.render_widget(Line::from(spans), area);
    }

    /// The keys that do something right now in the focused pane, so the line
    /// only lists what applies.
    pub(super) fn hints(&self) -> String {
        let mut hints: Vec<&str> = Vec::new();
        // What esc does in the focused pane, unless an open diff takes it.
        let mut esc = None;
        if self.view.in_preview {
            if self.view.diff().is_some() {
                hints.extend(self.comment_hints());
                hints.extend(self.diff_view_hints());
            } else {
                hints.push("y copy sha");
            }
            hints.push(if self.view.zoomed { "z unzoom" } else { "z zoom" });
            esc = Some("esc back");
        } else {
            match self.view.pane {
                Pane::Branches if self.branches.is_typing() => {
                    // Every other key is typed into the search, ? and q included.
                    // The search box says what esc does.
                    return "type to filter · enter check out".to_string();
                }
                Pane::Branches => {
                    hints.extend(["enter check out", "c compare", "space remotes", "i search"]);
                    if self.branches.is_searching() {
                        esc = Some("esc clear search");
                    }
                }
                Pane::Changes => {
                    let source = self.staging.selected_source();
                    match &source {
                        Some(DiffSource::Staged(_)) => hints.push("space unstage"),
                        Some(DiffSource::Conflict(_)) => hints.push("space mark resolved"),
                        Some(_) => hints.push("space stage"),
                        None => {}
                    }
                    if source.is_some() {
                        hints.push("a/A stage/unstage all");
                    }
                    hints.extend(self.enter_diff_hint(source.clone()));
                    if self.staging.can_restore() {
                        hints.push("d restore");
                    }
                    let (restore, delete) = self.staging.discardable();
                    if !restore.is_empty() || !delete.is_empty() {
                        hints.push("D discard all");
                    }
                    if self.staging.has_staged() {
                        hints.push("c commit");
                    }
                }
                Pane::Graph if self.graph.is_on_file() => {
                    hints.extend(self.enter_diff_hint(self.graph.selected_file_source()));
                    if self.view.diff().is_none() {
                        hints.push("n/N commit");
                    }
                    hints.extend([self.details_hint(), "y copy sha"]);
                    esc = Some("esc collapse");
                }
                Pane::Graph => {
                    let expanded = self.graph.is_selected_expanded();
                    hints.push(if expanded { "enter collapse" } else { "enter expand" });
                    if self.graph.is_on_stash() {
                        hints.extend(["a apply", "p pop", "d drop"]);
                    }
                    hints.extend([self.details_hint(), "c compare", "y copy sha", "b choose branches"]);
                    // Only worth it when open commits' files are in the way.
                    if self.graph.has_expanded() && self.view.diff().is_none() {
                        hints.push("n/N commit");
                    }
                    if expanded {
                        esc = Some("esc collapse");
                    } else if self.graph.has_expanded() {
                        esc = Some("esc collapse all");
                    }
                }
                Pane::Compare if self.compare.is_none() => {
                    hints.push("c choose two refs");
                    esc = Some("esc back to Repo");
                }
                Pane::Compare => {
                    let source = self.compare.as_ref().and_then(|compare| compare.selected_source());
                    hints.extend(self.enter_diff_hint(source));
                    if self.compare.as_ref().is_some_and(|compare| compare.stash_shown().is_some()) {
                        hints.extend(["h/l fold", "a apply", "p pop", "d drop"]);
                        esc = Some("esc back to stashes");
                    } else {
                        hints.extend(["h/l fold", "r swap from and to", "c new comparison"]);
                        esc = Some("esc close compare");
                    }
                }
                Pane::Review => {
                    hints.extend(["enter focus diff", "c edit", "d delete", "y copy all", "D clear"]);
                }
            }
            hints.extend(self.comment_hints());
            hints.extend(self.diff_view_hints());
        }
        if self.focused_file().is_some() {
            hints.push(self.editor_hint.as_str());
        }
        if !self.view.in_preview {
            if self.view.diff().is_some() {
                esc = Some("esc close diff");
            } else if self.view.details().is_some() {
                esc = Some("esc close details");
            }
        }
        hints.extend(esc);
        hints.join(" · ")
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

    /// i shows the selected commit's details, or moves into them if they show.
    pub(super) fn details_hint(&self) -> &'static str {
        let showing = self.view.details().map(|details| details.sha());
        if showing.is_some() && showing == self.graph.selected_sha() {
            "i focus details"
        } else {
            "i details"
        }
    }

    /// gc and gC, where they'd comment: on the diff's active line when it
    /// shows, and on its file whenever there's one to comment on.
    pub(super) fn comment_hints(&self) -> Vec<&'static str> {
        let Some(diff) = self.view.diff() else {
            return Vec::new();
        };
        if review::Place::of(&diff.source).is_none() {
            return Vec::new();
        }
        let mut hints = Vec::new();
        let on_line = diff.shows_active_line(self.view.in_preview) && diff.active_anchor().is_some();
        if on_line {
            hints.push(if diff.comment_at_active().is_some() { "gc edit comment" } else { "gc comment" });
        }
        let on_file = self.review.find(&diff.source, None).is_some();
        hints.push(if on_file { "gC edit file comment" } else { "gC comment file" });
        hints
    }

    /// The keys that work on a diff on screen, from any pane.
    pub(super) fn diff_view_hints(&self) -> Vec<&'static str> {
        let Some(diff) = self.view.diff() else {
            return Vec::new();
        };
        vec![
            "n/N change",
            if self.whole { "e changes only" } else { "e whole file" },
            if diff.is_split() { "s unified" } else { "s side by side" },
        ]
    }

    /// Enter on a file shows its diff, or moves into the diff if it's showing.
    pub(super) fn enter_diff_hint(&self, source: Option<DiffSource>) -> Option<&'static str> {
        let source = source?;
        let showing = self.view.diff().is_some_and(|diff| diff.source == source);
        Some(if showing { "enter focus diff" } else { "enter diff" })
    }
}

/// Hints like `space stage · enter diff` styled as popups show keys: each
/// hint's first word, its key, in the key colour, and the rest in the normal
/// text colour. "type to filter" has no key, so it's all normal.
pub(super) fn hint_spans(hints: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, hint) in hints.split(" · ").enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(ui::DIM)));
        }
        match hint.split_once(' ') {
            Some((key, action)) if key != "type" => {
                spans.push(Span::styled(key.to_string(), Style::default().fg(ui::KEY)));
                spans.push(Span::raw(format!(" {action}")));
            }
            _ => spans.push(Span::raw(hint.to_string())),
        }
    }
    spans
}
