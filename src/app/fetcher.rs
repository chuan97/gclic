//! Fetching the remotes the branches track, every minute, in the background,
//! so the Branches pane's ahead and behind counts stay current.

use crate::git::{self, Repo};
use anyhow::Result;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// How often the remotes the branches track are fetched in the background.
const FETCH_EVERY: Duration = Duration::from_secs(60);

pub(super) struct Fetcher {
    /// Whether the remotes are fetched every minute. On unless `:autofetch off`.
    pub(super) auto: bool,
    /// A fetch under way, on its own thread, as it can take seconds.
    running: Option<Receiver<Result<()>>>,
    /// When the last fetch started, to start the next a minute later.
    started: Option<Instant>,
    /// How the last fetch went: when it finished, or why it failed.
    last: Option<Result<Instant, String>>,
    /// What a fetch runs: git in the repo, except in tests.
    fetch: Arc<dyn Fn() -> Result<()> + Send + Sync>,
}

impl Fetcher {
    pub(super) fn new(repo: &Repo) -> Fetcher {
        let repo = repo.clone();
        Fetcher::with(move || git::background_fetch(&repo, &git::tracked_remotes(&repo)))
    }

    fn with(fetch: impl Fn() -> Result<()> + Send + Sync + 'static) -> Fetcher {
        Fetcher {
            auto: true,
            running: None,
            started: None,
            last: None,
            fetch: Arc::new(fetch),
        }
    }

    #[cfg(test)]
    fn is_fetching(&self) -> bool {
        self.running.is_some()
    }

    /// Fetches on a thread of its own, as it can take seconds, which would
    /// hold up the worker's refreshes.
    fn start(&mut self) {
        let (send, receive) = mpsc::channel();
        let fetch = Arc::clone(&self.fetch);
        thread::spawn(move || {
            let _ = send.send(fetch());
        });
        self.running = Some(receive);
        self.started = Some(Instant::now());
    }

    /// A fetch the user ran with `:git fetch`, which worked.
    pub(super) fn fetched(&mut self) {
        self.last = Some(Ok(Instant::now()));
    }

    /// Picks up a fetch that has ended, and starts the next when it's due. A
    /// fetch that brought anything moves the remote refs, which the next poll
    /// sees, refreshing everything. Whether one ended or started, which
    /// changes the note.
    pub(super) fn keep_fetching(&mut self) -> bool {
        let before = self.running.is_some();
        if let Some(running) = &self.running {
            match running.try_recv() {
                Ok(result) => {
                    self.last = Some(
                        result
                            .map(|()| Instant::now())
                            .map_err(|error| error.to_string()),
                    );
                    self.running = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.last = Some(Err("the fetch crashed".to_string()));
                    self.running = None;
                }
            }
        }
        let due = self
            .started
            .is_none_or(|started| started.elapsed() >= FETCH_EVERY);
        let ended = before && self.running.is_none();
        if self.auto && self.running.is_none() && due {
            self.start();
            return true;
        }
        ended
    }

    /// What the Branches title says about fetching: how long ago it last
    /// worked, or that it failed, and that it's fetching, before the first
    /// fetch ends or while the user's `:git fetch` runs. The fetches every
    /// minute don't say so, which would flicker.
    pub(super) fn note(&self, user_fetching: bool) -> Option<String> {
        let fetching = user_fetching || (self.running.is_some() && self.last.is_none());
        if fetching {
            return Some("fetching…".to_string());
        }
        match &self.last {
            None => None,
            Some(Ok(finished)) => Some(fetched_ago(finished.elapsed())),
            Some(Err(_)) => Some("fetch failed".to_string()),
        }
    }
}

/// How long ago, in minutes or hours, or under a minute in steps of ten
/// seconds, so it doesn't tick away at the edge of the eye.
fn fetched_ago(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..10 => "fetched just now".to_string(),
        seconds @ 10..60 => format!("fetched {}s ago", seconds / 10 * 10),
        seconds @ 60..3600 => format!("fetched {}m ago", seconds / 60),
        seconds => format!("fetched {}h ago", seconds / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fetcher whose fetches do `fetch` rather than run git.
    fn fake(fetch: fn() -> Result<()>) -> Fetcher {
        Fetcher::with(fetch)
    }

    /// Lets the fetch thread finish, then picks up its result.
    fn finish(fetcher: &mut Fetcher) {
        while fetcher.is_fetching() {
            thread::sleep(Duration::from_millis(1));
            fetcher.keep_fetching();
        }
    }

    #[test]
    fn the_fetch_note_says_how_long_ago() {
        let minutes = |m: u64| fetched_ago(Duration::from_secs(m * 60));
        assert_eq!(fetched_ago(Duration::from_secs(9)), "fetched just now");
        assert_eq!(fetched_ago(Duration::from_secs(10)), "fetched 10s ago");
        assert_eq!(fetched_ago(Duration::from_secs(59)), "fetched 50s ago");
        assert_eq!(minutes(3), "fetched 3m ago");
        assert_eq!(minutes(59), "fetched 59m ago");
        assert_eq!(minutes(130), "fetched 2h ago");
    }

    #[test]
    fn it_fetches_at_once_then_says_how_it_went() {
        let mut fetcher = fake(|| Ok(()));
        assert_eq!(fetcher.note(false), None);
        fetcher.keep_fetching();
        assert!(fetcher.is_fetching());
        assert_eq!(fetcher.note(false).as_deref(), Some("fetching…"));
        finish(&mut fetcher);
        assert_eq!(fetcher.note(false).as_deref(), Some("fetched just now"));
        // The next waits a minute.
        fetcher.keep_fetching();
        assert!(!fetcher.is_fetching());

        let mut failing = fake(|| Err(anyhow::anyhow!("no network")));
        failing.keep_fetching();
        finish(&mut failing);
        assert_eq!(failing.note(false).as_deref(), Some("fetch failed"));
    }

    #[test]
    fn autofetch_off_stops_the_fetches_every_minute() {
        let mut fetcher = fake(|| Ok(()));
        fetcher.auto = false;
        fetcher.keep_fetching();
        assert!(!fetcher.is_fetching());
        // The user's :git fetch still says it fetched.
        assert_eq!(fetcher.note(true).as_deref(), Some("fetching…"));
        fetcher.fetched();
        assert_eq!(fetcher.note(false).as_deref(), Some("fetched just now"));
    }

    #[test]
    fn only_the_first_fetch_every_minute_says_it_is_fetching() {
        let mut fetcher = fake(|| Ok(()));
        fetcher.keep_fetching();
        finish(&mut fetcher);
        fetcher.started = None;
        fetcher.keep_fetching();
        assert!(fetcher.is_fetching());
        assert_eq!(fetcher.note(false).as_deref(), Some("fetched just now"));
        finish(&mut fetcher);
    }
}
