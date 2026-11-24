//! Fetching the remotes the branches track, every minute, in the background,
//! so the Branches pane's ahead and behind counts stay current.

use crate::git;
use anyhow::Result;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// How often the remotes the branches track are fetched in the background.
const FETCH_EVERY: Duration = Duration::from_secs(60);

pub(super) struct Fetcher {
    /// Whether the remotes are fetched every minute. On unless `:fetch off`.
    pub(super) auto: bool,
    /// A fetch under way, on its own thread, as it can take seconds.
    running: Option<Receiver<Result<()>>>,
    /// When the last fetch started, to start the next a minute later.
    started: Option<Instant>,
    /// How the last fetch went: when it finished, or why it failed.
    last: Option<Result<Instant, String>>,
    /// What a fetch runs: git, except in tests.
    fetch: fn() -> Result<()>,
}

impl Fetcher {
    pub(super) fn new() -> Fetcher {
        Fetcher {
            auto: true,
            running: None,
            started: None,
            last: None,
            fetch: || git::background_fetch(&git::tracked_remotes()),
        }
    }

    pub(super) fn is_fetching(&self) -> bool {
        self.running.is_some()
    }

    /// Fetches on a thread of its own, as it can take seconds, which would
    /// hold up the worker's refreshes.
    pub(super) fn start(&mut self) {
        let (send, receive) = mpsc::channel();
        let fetch = self.fetch;
        thread::spawn(move || {
            let _ = send.send(fetch());
        });
        self.running = Some(receive);
        self.started = Some(Instant::now());
    }

    /// Picks up a fetch that has ended, and starts the next when it's due. A
    /// fetch that brought anything moves the remote refs, which the next poll
    /// sees, refreshing everything.
    pub(super) fn keep_fetching(&mut self) {
        if let Some(running) = &self.running {
            match running.try_recv() {
                Ok(result) => {
                    self.last = Some(result.map(|()| Instant::now()).map_err(|error| error.to_string()));
                    self.running = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.last = Some(Err("the fetch crashed".to_string()));
                    self.running = None;
                }
            }
        }
        let due = self.started.is_none_or(|started| started.elapsed() >= FETCH_EVERY);
        if self.auto && self.running.is_none() && due {
            self.start();
        }
    }

    /// What the Branches title says about fetching: how long ago it last
    /// worked, or that it failed. Nothing before the first fetch ends.
    pub(super) fn note(&self) -> Option<String> {
        match &self.last {
            None if self.running.is_some() => Some("fetching…".to_string()),
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
        Fetcher { fetch, ..Fetcher::new() }
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
        assert_eq!(fetcher.note(), None);
        fetcher.keep_fetching();
        assert!(fetcher.is_fetching());
        assert_eq!(fetcher.note().as_deref(), Some("fetching…"));
        finish(&mut fetcher);
        assert_eq!(fetcher.note().as_deref(), Some("fetched just now"));
        // The next waits a minute.
        fetcher.keep_fetching();
        assert!(!fetcher.is_fetching());

        let mut failing = fake(|| Err(anyhow::anyhow!("no network")));
        failing.keep_fetching();
        finish(&mut failing);
        assert_eq!(failing.note().as_deref(), Some("fetch failed"));
    }

    #[test]
    fn fetch_off_stops_the_fetches_every_minute_but_not_one_asked_for() {
        let mut fetcher = fake(|| Ok(()));
        fetcher.auto = false;
        fetcher.keep_fetching();
        assert!(!fetcher.is_fetching());
        fetcher.start();
        finish(&mut fetcher);
        assert_eq!(fetcher.note().as_deref(), Some("fetched just now"));
    }
}
