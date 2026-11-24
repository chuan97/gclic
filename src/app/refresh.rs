//! Refreshing: the repo read again, the background poll's and colouring's
//! results, and the open diff and details kept up to date.

use super::*;

impl App {
    /// Reloads everything from git, after an action or on `r`. Runs here
    /// rather than on the worker, so a change gclic made shows straight away.
    pub(super) fn refresh(&mut self) {
        self.generation += 1;
        let result = self.reload_panels();
        self.report(result);
        self.sync_diff();
        self.sync_details();
        self.request_colors();
        self.last_refresh = Instant::now();
    }

    /// The panes' refreshes don't depend on each other, so they run at once,
    /// along with the refs fingerprint: each is a few git calls, and most of
    /// their time is waiting for git. The graph's colours wait for the branch
    /// list, which the Branches pane fetches.
    pub(super) fn reload_panels(&mut self) -> Result<()> {
        let App { branches, staging, graph, compare, .. } = self;
        let (fingerprint, results) = thread::scope(|scope| {
            let fingerprint = scope.spawn(git::refs_fingerprint);
            let staged = scope.spawn(|| staging.refresh());
            let graphed = scope.spawn(|| graph.refresh());
            let compared = scope.spawn(|| compare.as_mut().map_or(Ok(()), |c| c.refresh()));
            let listed = branches.refresh();
            let crashed = || Err(anyhow::anyhow!("A refresh crashed"));
            let results = [
                listed,
                staged.join().unwrap_or_else(|_| crashed()),
                graphed.join().unwrap_or_else(|_| crashed()),
                compared.join().unwrap_or_else(|_| crashed()),
            ];
            (fingerprint.join().unwrap_or_default(), results)
        });
        self.refs_fingerprint = fingerprint;
        self.graph.color_by(self.branches.list());
        results.into_iter().collect()
    }

    /// Picks up what happened outside the app, such as git commands in another
    /// terminal. The worker does the reading, and `apply_poll` the rest.
    pub(super) fn auto_refresh(&mut self) {
        // Not while a popup or the command line is open, which switching
        // repos would throw away: it waits for the next poll after it.
        if matches!(self.modal, Modal::None)
            && let Ok(current) = std::env::current_dir()
            && let Some(root) = self.follow.as_mut().and_then(|follow| follow.moved_to(&current))
        {
            self.switch_repo(&root, false);
            let name = root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            self.message = format!("Followed Neovim to {name}");
        }
        let diff = self.view.diff().map(DiffPane::view);
        self.worker.poll(self.generation, diff);
        // Collects the editor commands that have finished, so none linger.
        self.editor_commands.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        self.last_refresh = Instant::now();
    }

    /// Takes in the worker's results. Whether there were any.
    pub(super) fn take_worker_results(&mut self) -> bool {
        let (results, alive) = self.worker.receive();
        let any = !results.is_empty() || !alive;
        for done in results {
            match done {
                Done::Poll { generation, fingerprint, status, diff } => {
                    self.apply_poll(generation, fingerprint, status, diff);
                }
                Done::Highlight { view, text, colored } => self.apply_highlight(view, text, colored),
            }
        }
        if !alive {
            self.worker = Worker::start();
            self.message = "The background worker stopped, so it was restarted".to_string();
        }
        any
    }

    /// Takes in what a poll found. The graph only reloads, with everything
    /// else, when a ref or HEAD moved.
    pub(super) fn apply_poll(
        &mut self,
        generation: u64,
        fingerprint: String,
        status: Result<git::Status>,
        diff: Option<(DiffView, Result<String>)>,
    ) {
        if generation != self.generation {
            return; // A refresh has run since, which read the repo more recently.
        }
        if fingerprint != self.refs_fingerprint {
            self.refresh();
            return;
        }
        match status {
            Ok(status) => self.staging.set_status(status),
            Err(error) => self.message = error.to_string(),
        }
        // While browsing Changes the diff follows the selection, which the new
        // status may have moved. Otherwise it takes the newer text, if any.
        let current = self.view.diff().map(|diff| diff.source.clone());
        let follows = self.view.focused(Pane::Changes) && current.as_ref().is_some_and(|s| s.is_working_tree());
        let selected = self.staging.selected_source();
        if follows && selected != current {
            match selected {
                Some(source) => self.show_diff(source),
                None => self.view.close_diff(),
            }
        } else if let Some(open) = self.view.diff_mut()
            && let Some((view, Ok(text))) = diff
            && open.view() == view
        {
            open.set_text(text);
        }
        self.request_colors();
    }

    pub(super) fn apply_highlight(&mut self, view: DiffView, text: String, colored: Colored) {
        if let Some(diff) = self.view.diff_mut()
            && diff.view() == view
            && diff.needs_colors()
        {
            // Only taken if the text matches, so not for the other kind of diff.
            diff.apply_colors(&text, colored.clone());
        }
        self.colors.insert(view, text, colored);
    }

    /// A diff as it would show now, whole or just the changes.
    pub(super) fn view_of(&self, source: &DiffSource) -> DiffView {
        DiffView { source: source.clone(), whole: self.whole }
    }

    /// Colours the open diff from the cache if it can, and asks the worker for
    /// the rest: the open diff if still uncoloured, the same file the other
    /// way, whole or just the changes, which e leads to, then every diff one
    /// key away in the focused list, so they're ready when the user gets
    /// there. Without a diff open, only the file under the cursor, one enter
    /// away. Asking again replaces what's still waiting.
    pub(super) fn request_colors(&mut self) {
        let mut wanted = Vec::new();
        if let Some(diff) = self.view.diff_mut() {
            let view = diff.view();
            if diff.needs_colors() {
                match self.colors.get(&view, diff.text()) {
                    Some(colored) => {
                        let text = diff.text().to_string();
                        diff.apply_colors(&text, colored);
                    }
                    None => wanted.push(view.clone()),
                }
            }
            let other = DiffView { whole: !view.whole, ..view };
            if !self.colors.contains(&other) {
                wanted.push(other);
            }
        }
        let mut around = match self.view.pane {
            Pane::Changes => self.staging.sources_around(),
            Pane::Graph => self.graph.file_sources_around(),
            Pane::Compare => self.compare.as_ref().map(|c| c.sources_around()).unwrap_or_default(),
            Pane::Branches | Pane::Review => Vec::new(),
        };
        if self.view.diff().is_none() {
            around.truncate(1);
        }
        for source in around {
            let view = self.view_of(&source);
            if !wanted.contains(&view) && !self.colors.contains(&view) {
                wanted.push(view);
            }
        }
        self.worker.highlight(wanted);
    }

    /// Brings an open diff up to date. While browsing Changes it follows the
    /// selection, because staging a file moves the selection to another one.
    pub(super) fn sync_diff(&mut self) {
        if self.view.diff().is_none() {
            return;
        }
        let shows_working_tree = self.view.diff().is_some_and(|diff| diff.source.is_working_tree());
        if self.view.focused(Pane::Changes) && shows_working_tree {
            match self.staging.selected_source() {
                Some(source) => self.show_diff(source),
                None => self.view.close_diff(),
            }
        } else if let Some(diff) = self.view.diff_mut() {
            let result = diff.reload();
            self.report(result);
        }
    }

    /// Keeps the details on the graph's selected commit, or the commit that
    /// the selected file belongs to.
    pub(super) fn sync_details(&mut self) {
        let Some(details) = self.view.details() else {
            return;
        };
        let Some(sha) = self.graph.selected_sha() else {
            self.close_preview();
            return;
        };
        if details.sha() != sha {
            match git::commit_details(sha) {
                Ok(details) => self.view.preview = Preview::Details(DetailsPane::new(details)),
                Err(error) => self.message = error.to_string(),
            }
        }
    }
}
