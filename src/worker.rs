//! A background thread for slow reads: polling the repo for changes and
//! colouring diffs. The UI hands it jobs and picks up the results when they
//! arrive, so keys never wait for them.
//!
//! Only reads happen here. Anything that changes the repo stays on the UI
//! thread, in order. Results say what they answer, so the UI can drop any
//! that are out of date by the time they arrive.

use crate::diff::{self, DiffView, Row};
use crate::git::{self, DiffSource, Status};
use anyhow::Result;
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

enum Job {
    /// What auto-refresh checks: the refs, the files' status, and the open diff.
    Poll { generation: u64, diff: Option<DiffSource> },
    /// A diff with its syntax colours.
    Highlight(DiffView),
}

pub enum Done {
    /// `generation` is the UI's count of changes when it asked, so it can tell
    /// whether the repo changed under the poll.
    Poll {
        generation: u64,
        fingerprint: String,
        status: Result<Status>,
        diff: Option<(DiffSource, Result<String>)>,
    },
    /// `text` is the diff the colours belong to.
    Highlight { view: DiffView, text: String, rows: Vec<Row> },
}

/// The jobs waiting. A poll goes before any highlighting, and there's at most
/// one, as a newer one makes an older one pointless.
#[derive(Default)]
struct Queue {
    poll: Option<Job>,
    highlights: VecDeque<DiffView>,
}

impl Queue {
    fn next(&mut self) -> Option<Job> {
        self.poll.take().or_else(|| self.highlights.pop_front().map(Job::Highlight))
    }
}

/// The queue, shared with the thread, and the signal that wakes it.
type Shared = Arc<(Mutex<Queue>, Condvar)>;

pub struct Worker {
    shared: Shared,
    results: Receiver<Done>,
}

impl Worker {
    pub fn start() -> Worker {
        let shared: Shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (sender, results) = mpsc::channel();
        let queue = Arc::clone(&shared);
        thread::spawn(move || work(&queue, &sender));
        Worker { shared, results }
    }

    /// Edits the queue and wakes the thread.
    fn with_queue(&self, edit: impl FnOnce(&mut Queue)) {
        let (queue, wake) = &*self.shared;
        // A panic on the thread poisons the lock, but the queue stays usable.
        let mut queue = queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        edit(&mut queue);
        wake.notify_one();
    }

    /// Asks for a poll, replacing one still waiting.
    pub fn poll(&self, generation: u64, diff: Option<DiffSource>) {
        self.with_queue(|queue| queue.poll = Some(Job::Poll { generation, diff }));
    }

    /// Replaces the diffs waiting to be coloured, most wanted first. Whatever
    /// was waiting is for where the cursor used to be, so it's dropped. A diff
    /// already being coloured is finished, and still useful to the cache.
    pub fn highlight(&self, views: Vec<DiffView>) {
        self.with_queue(|queue| queue.highlights = views.into());
    }

    /// The results that have arrived, without waiting. False once the thread
    /// has stopped, which only a panic on it would cause.
    pub fn receive(&self) -> (Vec<Done>, bool) {
        let mut results = Vec::new();
        loop {
            match self.results.try_recv() {
                Ok(done) => results.push(done),
                Err(TryRecvError::Empty) => return (results, true),
                Err(TryRecvError::Disconnected) => return (results, false),
            }
        }
    }
}

/// The thread's loop: wait for a job, do it, send the result.
fn work(shared: &Shared, sender: &Sender<Done>) {
    loop {
        let job = {
            let (queue, wake) = &**shared;
            let mut queue = queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                if let Some(job) = queue.next() {
                    break job;
                }
                queue = wake.wait(queue).unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let Some(done) = run(job) else {
            continue;
        };
        if sender.send(done).is_err() {
            return; // The app has quit.
        }
    }
}

fn run(job: Job) -> Option<Done> {
    match job {
        // Three separate git calls, so they run at once.
        Job::Poll { generation, diff } => Some(thread::scope(|scope| {
            let fingerprint = scope.spawn(git::refs_fingerprint);
            let diff = scope.spawn(|| {
                diff.map(|source| {
                    let text = git::diff(&source);
                    (source, text)
                })
            });
            let status = git::status();
            Done::Poll {
                generation,
                fingerprint: fingerprint.join().unwrap_or_default(),
                status,
                diff: diff.join().unwrap_or(None),
            }
        })),
        Job::Highlight(view) => {
            // Without its diff there's nothing to colour, and the UI shows the
            // error when it loads the diff itself.
            let text = git::diff_with(&view.source, view.whole).ok()?;
            let mut rows = diff::rows_for(&view.source, &text);
            diff::highlight_rows(&mut rows, &view.source);
            Some(Done::Highlight { view, text, rows })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> DiffView {
        DiffView { source: DiffSource::Unstaged(path.to_string()), whole: false }
    }

    #[test]
    fn a_poll_goes_before_waiting_highlights() {
        let mut queue = Queue { poll: None, highlights: VecDeque::from([file("a")]) };
        queue.poll = Some(Job::Poll { generation: 1, diff: None });
        assert!(matches!(queue.next(), Some(Job::Poll { .. })));
        assert!(matches!(queue.next(), Some(Job::Highlight(view)) if view == file("a")));
        assert!(queue.next().is_none());
    }

    #[test]
    fn new_highlights_replace_the_waiting_ones() {
        let worker = Worker { shared: Arc::default(), results: mpsc::channel().1 };
        worker.highlight(vec![file("a"), file("b")]);
        worker.highlight(vec![file("c")]);
        let (queue, _) = &*worker.shared;
        let waiting: Vec<_> = queue.lock().unwrap().highlights.iter().cloned().collect();
        assert_eq!(waiting, [file("c")]);
    }

    #[test]
    fn colours_a_diff_in_the_background() {
        // gclic's own first commit adds a src/main.rs, which is Rust to colour.
        let first = git::log(&["HEAD".to_string()], 10_000).unwrap().pop().unwrap();
        let source = DiffSource::Commit { sha: first.sha, path: "src/main.rs".to_string() };
        let worker = Worker::start();
        worker.highlight(vec![DiffView { source: source.clone(), whole: false }]);
        let done = worker.results.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        let Done::Highlight { view, rows, .. } = done else { panic!("expected colours") };
        assert_eq!(view.source, source);
        let colored = rows.iter().any(|row| matches!(row, Row::Lines { right: Some(cell), .. } if !cell.colors.is_empty()));
        assert!(colored);
    }
}
