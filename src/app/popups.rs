//! The popups: confirmations, the commit box, the pickers and what a pick does,
//! and drawing them.

use super::*;

impl App {
    /// Like quitting: enter goes ahead, esc cancels, and other keys are ignored
    /// so that a stray one can't do either.
    pub(super) fn handle_confirm_key(&mut self, confirm: Confirm, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let result = match &confirm {
                    Confirm::Restore(path) => return self.restore_file(path),
                    Confirm::RestoreChange { path, patch } => {
                        return self.restore_change(path, patch);
                    }
                    Confirm::DiscardAll { restore, delete } => {
                        return self.discard_all(restore, delete);
                    }
                    Confirm::DeleteUntracked(path) => git::delete_untracked(&self.repo, path),
                    Confirm::CloseCompare => {
                        self.close_compare();
                        return;
                    }
                    Confirm::DropStash(stash) => {
                        let result = git::stash_drop(&self.repo, stash);
                        self.after_stash(StashAction::Drop, stash, result);
                        return;
                    }
                    Confirm::DeleteComment(index) => {
                        self.review.remove(*index);
                        self.message = "Comment deleted".to_string();
                        self.after_review_change();
                        self.show_selected_comment();
                        return;
                    }
                    Confirm::ClearReview => {
                        self.review.clear();
                        self.after_review_change();
                        self.message = "Cleared the review".to_string();
                        return;
                    }
                };
                self.apply(Action::after_change(result));
            }
            KeyCode::Esc => {}
            _ => self.modal = Modal::Confirm(confirm),
        }
    }

    pub(super) fn handle_commit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Esc keeps the draft for next time.
            KeyCode::Esc => return,
            // ctrl-s is kept for terminals that send ctrl-enter as plain enter.
            KeyCode::Enter | KeyCode::Char('s') if ctrl => {
                if self.commit_draft.trim().is_empty() {
                    self.message = "The commit message is empty".to_string();
                } else {
                    match git::commit(&self.repo, &self.commit_draft) {
                        Ok(()) => {
                            // What was staged is committed, which unstaging
                            // it now would only confuse.
                            self.history.clear();
                            self.commit_draft.clear();
                            self.refresh();
                            self.message = "Committed".to_string();
                        }
                        // The draft stays, so a failed hook costs no retyping.
                        Err(error) => self.modal = Modal::Error(error.to_string()),
                    }
                    return;
                }
            }
            KeyCode::Enter => self.commit_draft.push('\n'),
            KeyCode::Backspace => {
                self.commit_draft.pop();
            }
            KeyCode::Char(ch) if !ctrl => self.commit_draft.push(ch),
            _ => {}
        }
        self.modal = Modal::CommitBox;
    }

    pub(super) fn handle_picker_key(
        &mut self,
        mut picker: Picker,
        usage: PickerUse,
        key: KeyEvent,
    ) {
        // `p` pastes a ref, so a sha yanked in the graph is two keys from comparing.
        let paste = key.code == KeyCode::Char('p') && key.modifiers == KeyModifiers::NONE;
        if paste && picker.accepts_free_text() && !picker.is_typing() {
            match read_clipboard() {
                Ok(Some(text)) => picker.set_query(text),
                Ok(None) => self.message = "The clipboard is empty".to_string(),
                Err(error) => {
                    self.modal = Modal::Error(error.to_string());
                    return;
                }
            }
            self.modal = Modal::Picker(picker, usage);
            return;
        }
        // A stash's keys, outside the search, where they'd be letters.
        if let PickerUse::Stash(stashes) = &usage
            && !picker.is_typing()
        {
            let action = StashAction::from_key(key.code);
            let highlighted = picker.highlighted();
            let stash = stashes
                .iter()
                .find(|(label, _)| Some(label) == highlighted)
                .map(|(_, stash)| stash.clone());
            if let (Some(action), Some(stash)) = (action, stash) {
                self.act_on_stash(action, stash);
                return;
            }
        }
        match picker.handle_key(key) {
            Outcome::Open => self.modal = Modal::Picker(picker, usage),
            Outcome::Cancelled => {}
            Outcome::Picked(picked) => self.use_picked(usage, picked),
        }
    }

    pub(super) fn use_picked(&mut self, usage: PickerUse, picked: Vec<String>) {
        // Refs have no spaces, so this also takes the sha out of a yank's label.
        let first = picked
            .first()
            .and_then(|item| item.split_whitespace().next());
        let first = first.unwrap_or_default().to_string();
        match usage {
            PickerUse::Stash(stashes) => {
                let picked = picked.first();
                if let Some((_, stash)) =
                    stashes.into_iter().find(|(label, _)| Some(label) == picked)
                {
                    self.open_stash(stash);
                }
            }
            PickerUse::Repo(repos) => {
                let picked = picked.first();
                match repos.into_iter().find(|(label, _)| Some(label) == picked) {
                    // The search moves up for the rest of the session.
                    Some((label, parent)) if label == repos::UP => {
                        self.search_root = parent;
                        self.open_repo_picker("");
                    }
                    Some((_, repo)) => self.switch_repo(&repo, true),
                    None => {}
                }
            }
            PickerUse::GraphBranches => {
                // Confirming the default view's branches as they were keeps the
                // default view, which follows a checkout to another branch.
                let unchanged =
                    self.graph.is_default_view() && same_refs(&picked, &self.graph.shown_refs());
                let refs = if unchanged { Vec::new() } else { picked };
                let result = self.graph.set_refs(refs, self.branches.list());
                self.report(result);
            }
            PickerUse::CompareA => {
                if git::resolve(&self.repo, &first).is_none() {
                    self.modal = Modal::Error(format!("Unknown ref: {first}"));
                    return;
                }
                self.open_compare_b_picker(git::display_ref(&self.repo, &first));
            }
            PickerUse::CompareB { a } => {
                if git::resolve(&self.repo, &first).is_none() {
                    self.modal = Modal::Error(format!("Unknown ref: {first}"));
                    return;
                }
                self.start_compare(a, git::display_ref(&self.repo, &first));
            }
        }
    }

    /// Asks for the ref to compare `a` with.
    pub(super) fn open_compare_b_picker(&mut self, a: String) {
        match compare_items(&self.repo, &self.recent_yanks) {
            Ok(mut items) => {
                // Comparing a ref with itself shows nothing.
                items.retain(|item| item.split_whitespace().next() != Some(a.as_str()));
                let title = format!("Compare from {a}: to");
                let picker = Picker::with_free_text(&title, items).with_hints(&COMPARE_PICKER_KEYS);
                self.modal = Modal::Picker(picker, PickerUse::CompareB { a });
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    pub(super) fn open_graph_picker(&mut self) {
        match git::branches(&self.repo) {
            Ok(branches) => {
                let names = branches.into_iter().map(|b| b.name).collect();
                // The branches on screen start ticked, the default view's too.
                let ticked = self.graph.shown_refs();
                let hints = [
                    ("space", "tick"),
                    ("a/A", "all, none"),
                    ("enter", "apply"),
                    ("esc", "close"),
                ];
                let picker = Picker::multi("Graph branches", names, ticked).with_hints(&hints);
                self.modal = Modal::Picker(picker, PickerUse::GraphBranches);
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    pub(super) fn open_compare_picker(&mut self) {
        match compare_items(&self.repo, &self.recent_yanks) {
            Ok(items) => {
                let picker =
                    Picker::with_free_text("Compare: from", items).with_hints(&COMPARE_PICKER_KEYS);
                self.modal = Modal::Picker(picker, PickerUse::CompareA);
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    pub(super) fn render_modal(&mut self, frame: &mut Frame) {
        let area = frame.area();
        match &mut self.modal {
            // The command line is drawn in place of the hints.
            Modal::None | Modal::Command(_) => {}
            Modal::Output(output) => output.render(frame, area),
            Modal::Picker(picker, _) => picker.render(frame, area),
            Modal::Confirm(confirm) => {
                let bold = Style::default().add_modifier(Modifier::BOLD);
                let (title, body, action) = match confirm {
                    Confirm::Restore(path) => (
                        " Discard changes ",
                        vec![Line::from(vec![
                            Span::raw("Discard your changes to "),
                            Span::styled(path.clone(), bold),
                            Span::raw("?"),
                        ])],
                        "discard",
                    ),
                    Confirm::RestoreChange { path, .. } => (
                        " Discard change ",
                        vec![
                            Line::from(vec![
                                Span::raw("Discard this change to "),
                                Span::styled(path.clone(), bold),
                                Span::raw("?"),
                            ]),
                            Line::styled(
                                "The file's other changes stay. This can't be undone.",
                                Style::default().fg(ui::DIM),
                            ),
                        ],
                        "discard",
                    ),
                    Confirm::DiscardAll { restore, delete } => {
                        let files = |count: usize, kind: &str| match count {
                            1 => format!("1 {kind}file"),
                            n => format!("{n} {kind}files"),
                        };
                        let what = match (restore.len(), delete.len()) {
                            (r, 0) => format!("Discard your changes to {}?", files(r, "")),
                            (0, d) => format!("Delete {}?", files(d, "untracked ")),
                            (r, d) => format!(
                                "Discard your changes to {} and delete {}?",
                                files(r, ""),
                                files(d, "untracked ")
                            ),
                        };
                        (
                            " Discard all ",
                            vec![
                                Line::styled(what, bold),
                                Line::styled(
                                    "Staged changes stay. This can't be undone.",
                                    Style::default().fg(ui::DIM),
                                ),
                            ],
                            "discard",
                        )
                    }
                    Confirm::DropStash(stash) => (
                        " Drop stash ",
                        vec![
                            Line::from(vec![
                                Span::raw("Drop "),
                                Span::styled(stash.name.clone(), bold),
                                Span::raw("?"),
                            ]),
                            Line::styled(stash.message.clone(), Style::default().fg(ui::DIM)),
                        ],
                        "drop",
                    ),
                    Confirm::DeleteUntracked(path) => (
                        " Delete file ",
                        vec![Line::from(vec![
                            Span::styled(path.clone(), bold),
                            Span::raw(" is untracked. Permanently delete it?"),
                        ])],
                        "delete",
                    ),
                    Confirm::DeleteComment(index) => {
                        let comment = self.review.get(*index);
                        let location = comment
                            .map(|comment| comment.location())
                            .unwrap_or_default();
                        let first = comment
                            .and_then(|comment| comment.body.lines().next())
                            .unwrap_or_default();
                        let body = vec![
                            Line::from(vec![
                                Span::raw("Delete the comment on "),
                                Span::styled(location, bold),
                                Span::raw("?"),
                            ]),
                            Line::styled(first.to_string(), Style::default().fg(ui::DIM)),
                        ];
                        (" Delete comment ", body, "delete")
                    }
                    Confirm::ClearReview => {
                        let count = match self.review.len() {
                            1 => "the review's comment".to_string(),
                            n => format!("the review's {n} comments"),
                        };
                        (
                            " Clear review ",
                            vec![Line::raw(format!("Delete {count}?"))],
                            "clear",
                        )
                    }
                    Confirm::CloseCompare => {
                        let refs = self.compare.as_ref().map(|c| c.refs()).unwrap_or_default();
                        let body = vec![
                            Line::from(vec![
                                Span::raw("Close the comparison "),
                                Span::styled(refs, bold),
                                Span::raw("?"),
                            ]),
                            Line::from(vec![
                                Span::styled("tab", Style::default().fg(ui::KEY)),
                                Span::raw(" goes to Repo and keeps it."),
                            ]),
                        ];
                        (" Close comparison ", body, "close")
                    }
                };
                let hints = ui::key_hints(&[("enter", action), ("esc", "cancel")], Some(action));
                // The borders, plus a row in case a long path or ref wraps.
                let height = body.len() as u16 + 3;
                render_popup(frame, area, title, body, hints, (56, height), Color::Cyan);
            }
            Modal::ConfirmQuit => {
                // Quitting loses whatever was open: the graph's branches, the
                // comparison, expanded commits. gclic starts from scratch.
                let hints = ui::key_hints(&[("enter", "quit"), ("esc", "cancel")], Some("quit"));
                let mut body = vec![Line::raw("Quit gclic?")];
                // gclic doesn't keep comments: they'd be lost.
                let lost = match self.review.len() {
                    0 => None,
                    1 => Some("Your review's comment will be lost.".to_string()),
                    n => Some(format!("Your review's {n} comments will be lost.")),
                };
                body.extend(
                    lost.map(|lost| Line::styled(lost, Style::default().fg(ui::warning()))),
                );
                let height = body.len() as u16 + 3;
                render_popup(
                    frame,
                    area,
                    " Quit ",
                    body,
                    hints,
                    (44, height),
                    Color::Cyan,
                );
            }
            Modal::CommentBox(comment) => {
                let location = review::location(comment.source.path(), &comment.anchor);
                // The line it's on, above, or nothing for the whole file.
                let quoted = comment
                    .anchor
                    .as_ref()
                    .map(|anchor| anchor.text.trim().to_string());
                let hints = ui::key_hints(&[("ctrl-enter", "save"), ("esc", "cancel")], None);
                let title = format!(" Comment on {location} ");
                render_text_box(frame, area, &title, quoted, &comment.draft, hints);
            }
            Modal::CommitBox => {
                let hints = ui::key_hints(
                    &[("ctrl-enter", "commit"), ("esc", "close, keeping it")],
                    None,
                );
                render_text_box(
                    frame,
                    area,
                    " Commit message ",
                    None,
                    &self.commit_draft,
                    hints,
                );
            }
            Modal::Error(error) => {
                let body: Vec<Line> = error
                    .lines()
                    .map(|line| Line::raw(line.to_string()))
                    .collect();
                let height = (body.len() as u16 + 3).min(20);
                let hints = ui::key_hints(&[("any key", "closes")], None);
                render_popup(
                    frame,
                    area,
                    " Git error ",
                    body,
                    hints,
                    (72, height),
                    ui::error(),
                );
            }
            Modal::Help(help) => help.render(frame, area),
        }
    }
}

/// What the picker's result will be used for.
pub(super) enum PickerUse {
    GraphBranches,
    CompareA,
    CompareB {
        a: String,
    },
    /// `:cd`: each repo's label, and where it is.
    Repo(Vec<(String, PathBuf)>),
    /// `:stash`: each stash's label, and the stash.
    Stash(Vec<(String, Stash)>),
}

/// The popup currently on top of everything, which gets all the keys.
pub(super) enum Modal {
    None,
    Picker(Picker, PickerUse),
    Confirm(Confirm),
    CommitBox,
    CommentBox(CommentBox),
    ConfirmQuit,
    Error(String),
    Help(HelpPopup),
    /// The command line, opened with `:`, holding what's typed so far.
    Command(String),
    /// What a `:git` command printed.
    Output(OutputPopup),
}

/// Whether two lists hold the same refs, in any order.
pub(super) fn same_refs(a: &[String], b: &[String]) -> bool {
    let (mut a, mut b) = (a.to_vec(), b.to_vec());
    a.sort();
    b.sort();
    a == b
}

/// A popup in the middle of the screen: its title says what it is, and its
/// bottom border lists the keys, as `ui::key_hints` makes them. It's wide
/// enough for those keys, even if `width` is less. The border is cyan, so the
/// keys' yellow and a dangerous action's red stand out from it, except for an
/// error's, which is red.
pub(super) fn render_popup(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    body: Vec<Line<'static>>,
    hints: Line<'static>,
    (width, height): (u16, u16),
    color: Color,
) {
    let width = width.max(hints.width() as u16 + 4);
    let popup = ui::centered(area, width, height);
    frame.render_widget(Clear, popup);
    let block = ui::panel(title.to_string(), true)
        .border_style(Style::default().fg(color))
        .title_bottom(hints);
    let paragraph = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

/// Where a commit message or a comment is typed: `draft`, wrapped, with the
/// cursor at its end, under a dimmed line `above` it, if any. The box grows
/// with the text, up to most of the screen's height, and then scrolls to
/// keep the cursor in view.
pub(super) fn render_text_box(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    above: Option<String>,
    draft: &str,
    hints: Line<'static>,
) {
    const WIDTH: u16 = 72;
    const MIN_HEIGHT: u16 = 12;
    let width = WIDTH.max(hints.width() as u16 + 4).min(area.width);
    let text_width = width.saturating_sub(2) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(above) = above {
        let dim = Style::default().fg(ui::DIM);
        lines.extend(
            ui::wrap(&above, text_width)
                .into_iter()
                .map(|line| Line::styled(line, dim)),
        );
        lines.push(Line::default());
    }
    let typed = format!("{draft}▏");
    lines.extend(
        typed
            .split('\n')
            .flat_map(|line| ui::wrap(line, text_width))
            .map(Line::raw),
    );
    let most = (area.height * 4 / 5).max(MIN_HEIGHT.min(area.height));
    let height = (lines.len() as u16 + 2).clamp(MIN_HEIGHT.min(most), most);
    let room = height.saturating_sub(2) as usize;
    let hidden = lines.len().saturating_sub(room);
    let popup = ui::centered(area, width, height);
    frame.render_widget(Clear, popup);
    let block = ui::panel(title.to_string(), true)
        .border_style(Style::default().fg(Color::Cyan))
        .title_bottom(hints);
    frame.render_widget(Paragraph::new(lines.split_off(hidden)).block(block), popup);
}

/// Keeps the last two yanks, the most a comparison can use. In yank order, so
/// that the first one yanked is the top choice for A and the second for B.
pub(super) fn remember_yank(yanks: &mut Vec<String>, label: String) {
    let sha = label.split_whitespace().next().unwrap_or_default();
    yanks.retain(|yank| yank.split_whitespace().next() != Some(sha));
    yanks.push(label);
    if yanks.len() > 2 {
        yanks.remove(0);
    }
}

pub(super) fn compare_items(repo: &Repo, recent_yanks: &[String]) -> Result<Vec<String>> {
    let mut items = recent_yanks.to_vec();
    items.push("HEAD".to_string());
    items.extend(git::branches(repo)?.into_iter().map(|b| b.name));
    items.extend(git::tags(repo)?);
    Ok(items)
}
