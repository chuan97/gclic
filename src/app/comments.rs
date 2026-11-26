//! Review comments: writing them on a diff's lines, and the Review tab's list.

use super::*;

impl App {
    /// gc: writes a comment on the diff's active line, or edits the one
    /// already on it. From another pane too, once n or N shows the active
    /// line there, so a change can be commented on without entering it.
    pub(super) fn comment_on_line(&mut self) {
        let Some(diff) = self.view.diff() else {
            self.message = "Open a diff to comment on its lines".to_string();
            return;
        };
        if !diff.shows_active_line(self.view.in_preview) {
            self.message = "Step to a change with n, or enter the diff, to comment on a line".to_string();
            return;
        }
        if review::Place::of(&diff.source).is_none() {
            self.message = "A conflict takes no comments: o resolves it in your editor".to_string();
            return;
        }
        let Some(anchor) = diff.comment_at_active().or_else(|| diff.active_anchor()) else {
            self.message = "That's a heading: gC comments on the whole file".to_string();
            return;
        };
        let source = diff.source.clone();
        self.open_comment_box(source, Some(anchor));
    }

    /// gC: writes a comment on the whole file in the diff, or edits it.
    pub(super) fn comment_on_file(&mut self) {
        let Some(diff) = self.view.diff() else {
            self.message = "Open a file's diff to comment on it".to_string();
            return;
        };
        if review::Place::of(&diff.source).is_none() {
            self.message = "A conflict takes no comments: o resolves it in your editor".to_string();
            return;
        }
        let source = diff.source.clone();
        self.open_comment_box(source, None);
    }

    /// The comment box, holding the comment already there, if any.
    pub(super) fn open_comment_box(&mut self, source: DiffSource, anchor: Option<Anchor>) {
        let key = anchor.as_ref().map(|anchor| (anchor.side, anchor.number));
        let existing = self.review.find(&source, key).and_then(|index| self.review.get(index));
        let draft = existing.map(|comment| comment.body.clone()).unwrap_or_default();
        self.modal = Modal::CommentBox(CommentBox { source, anchor, draft });
    }

    /// Typing a comment: ctrl-enter saves it, an empty one removing what was
    /// there, and esc leaves it as it was.
    pub(super) fn handle_comment_key(&mut self, mut comment: CommentBox, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return,
            // ctrl-s for terminals that send ctrl-enter as plain enter.
            KeyCode::Enter | KeyCode::Char('s') if ctrl => {
                let had = self.review.len();
                let CommentBox { source, anchor, draft } = comment;
                if let Some(comment) = Comment::new(&self.repo, source, anchor, draft) {
                    self.review.set(comment);
                }
                self.message = match (had, self.review.len()) {
                    (before, after) if after < before => "Comment removed".to_string(),
                    (0, 1) => "Comment saved: tab goes to the Review tab, to copy it".to_string(),
                    (before, after) if after > before => format!("Comment saved: {after} in the review"),
                    _ => "Comment saved".to_string(),
                };
                self.after_review_change();
                return;
            }
            KeyCode::Enter => comment.draft.push('\n'),
            KeyCode::Backspace => {
                comment.draft.pop();
            }
            KeyCode::Char(ch) if !ctrl => comment.draft.push(ch),
            _ => {}
        }
        self.modal = Modal::CommentBox(comment);
    }

    pub(super) fn act_on_review(&mut self, action: ListAction) {
        match action {
            ListAction::None => {}
            ListAction::Moved => self.show_selected_comment(),
            ListAction::Enter => {
                if self.view.diff().is_none() {
                    self.show_selected_comment();
                }
                if self.view.diff().is_some() {
                    self.focus_preview();
                }
            }
            ListAction::Edit(index) => {
                if let Some(comment) = self.review.get(index) {
                    let (source, anchor, draft) = (comment.source.clone(), comment.anchor.clone(), comment.body.clone());
                    self.modal = Modal::CommentBox(CommentBox { source, anchor, draft });
                }
            }
            ListAction::Delete(index) => self.modal = Modal::Confirm(Confirm::DeleteComment(index)),
            ListAction::Copy => match copy_to_clipboard(&self.review.text()) {
                Ok(()) => {
                    let count = match self.review.len() {
                        1 => "the comment".to_string(),
                        n => format!("the {n} comments"),
                    };
                    self.message = format!("Copied {count}, ready to paste");
                }
                Err(error) => self.modal = Modal::Error(error.to_string()),
            },
            ListAction::Clear => self.modal = Modal::Confirm(Confirm::ClearReview),
        }
    }

    /// Shows the selected comment's diff, at its line.
    pub(super) fn show_selected_comment(&mut self) {
        let Some(comment) = self.review_list.selected(&self.review).and_then(|index| self.review.get(index)) else {
            return;
        };
        let anchor = comment.anchor.clone();
        let source = self.review_source(comment);
        self.show_diff(source);
        if let Some(diff) = self.view.diff_mut() {
            diff.go_to(anchor.as_ref());
        }
    }

    /// The diff to show a comment in. A working-tree file's changes may have
    /// been staged since, or the other way round: the diff that has it now.
    pub(super) fn review_source(&self, comment: &Comment) -> DiffSource {
        if comment.source.is_working_tree() {
            self.staging.source_for_path(comment.path(), &comment.source).unwrap_or_else(|| comment.source.clone())
        } else {
            comment.source.clone()
        }
    }

    /// Once the last comment goes, so does the Review tab.
    pub(super) fn after_review_change(&mut self) {
        if !self.review.is_empty() {
            return;
        }
        if self.tab == Tab::Review {
            self.go_to_tab(Tab::Repo);
        }
        self.saved_views[Tab::Review as usize] = None;
        self.review_list = ReviewList::new();
    }
}

/// A review comment being written, on the diff's line it's for, or on the
/// whole file for None.
pub(super) struct CommentBox {
    pub(super) source: DiffSource,
    pub(super) anchor: Option<Anchor>,
    pub(super) draft: String,
}
