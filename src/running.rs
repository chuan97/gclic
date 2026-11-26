//! A `:git` command run in the background, its output captured, and the
//! popup that shows the output, with advice when it failed.

use crate::ansi::{ansi_line, clean_output, strip_ansi};
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

/// Whether a command failed for want of a terminal: for an editor, a
/// password, or a passphrase, which `:git` turns off.
pub fn needed_terminal(lines: &[String]) -> bool {
    const SIGNS: [&str; 4] = [
        "problem with the editor",
        "terminal prompts disabled",
        "could not read Username",
        "could not read Password",
    ];
    lines
        .iter()
        .map(|line| strip_ansi(line))
        .any(|line| SIGNS.iter().any(|sign| line.contains(sign)))
}

/// A command git suggests in its output, to run as it is: one on a line of its
/// own, indented, as in "To push the current branch … use" followed by
/// `    git push --set-upstream origin main`. Ones with placeholders, such as
/// `<branch>`, are left out, as they'd need filling in.
pub fn suggested_command(lines: &[String]) -> Option<String> {
    lines.iter().map(|line| strip_ansi(line)).find_map(|line| {
        let indented = line.starts_with(char::is_whitespace);
        let command = line.trim();
        let runnable = command.starts_with("git ")
            && !command.contains(['<', '>'])
            && !command.contains("...");
        (indented && runnable).then(|| command.to_string())
    })
}

/// What a command left when it ended.
pub struct Finished {
    pub status: Option<ExitStatus>,
    pub lines: Vec<String>,
}

impl Finished {
    pub fn succeeded(&self) -> bool {
        self.status.is_some_and(|status| status.success())
    }
}

/// A command running in the background, with its output collected as it goes.
pub struct Running {
    /// What was typed, as shown while it runs and on its output.
    pub label: String,
    child: Arc<Mutex<Child>>,
    finished: Receiver<Finished>,
    /// Whether the user stopped it, which isn't worth a popup.
    pub stopped: bool,
}

impl Running {
    /// Starts `command` with no input and its output captured.
    pub fn start(label: String, mut command: Command) -> std::io::Result<Running> {
        // Its own process group, so stopping it stops what it started too, and
        // it can't take keys from gclic's terminal.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let child = Arc::new(Mutex::new(child));
        let (send, finished) = mpsc::channel();
        let waiting = Arc::clone(&child);
        thread::spawn(move || {
            // Both at once, so neither fills its pipe while the other is read.
            let errors = thread::spawn(move || read_all(stderr));
            let mut bytes = read_all(stdout);
            bytes.extend(errors.join().unwrap_or_default());
            // The output ends when the command does, so this doesn't wait long,
            // and stopping it meanwhile can take the lock.
            let status = waiting.lock().ok().and_then(|mut child| child.wait().ok());
            let _ = send.send(Finished {
                status,
                lines: clean_output(&bytes),
            });
        });
        Ok(Running {
            label,
            child,
            finished,
            stopped: false,
        })
    }

    /// The result, once it has ended.
    pub fn finished(&self) -> Option<Finished> {
        self.finished.try_recv().ok()
    }

    /// Stops it, and whatever it started: git runs helpers, such as the one
    /// that fetches over HTTPS, which would otherwise keep running, holding
    /// its output open.
    pub fn stop(&mut self) {
        self.stopped = true;
        let Ok(mut child) = self.child.lock() else {
            return;
        };
        let pid = child.id().to_string();
        let tree = if cfg!(windows) {
            Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid])
                .output()
        } else {
            // The group has the command's id, as it leads it.
            Command::new("kill")
                .args(["-TERM", "--", &format!("-{pid}")])
                .output()
        };
        if !tree.is_ok_and(|output| output.status.success()) {
            let _ = child.kill();
        }
    }
}

fn read_all(pipe: Option<impl Read>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut bytes);
    }
    bytes
}

/// A command to run next, after one that failed, as typed after `:`.
enum Advice {
    /// The same command, in the terminal, as it asked for something.
    Terminal(String),
    /// What git suggested.
    Git(String),
}

/// What a `:git` command printed, in a popup to scroll through. Red when the
/// command failed.
pub struct OutputPopup {
    title: String,
    lines: Vec<String>,
    failed: bool,
    /// gclic's own advice, after the command's output, which is then dimmed
    /// to set the two apart.
    advice: Option<Advice>,
    scroll: ui::Scroll,
}

impl OutputPopup {
    pub fn new(label: &str, finished: Finished) -> OutputPopup {
        let failed = !finished.succeeded();
        let mut lines = finished.lines;
        if lines.is_empty() {
            lines.push("(no output)".to_string());
        }
        let advice = if !failed {
            None
        } else if needed_terminal(&lines) {
            Some(Advice::Terminal(format!("!{label}")))
        } else {
            suggested_command(&lines).map(Advice::Git)
        };
        OutputPopup {
            title: format!(" {label} "),
            lines,
            failed,
            advice,
            scroll: ui::Scroll::default(),
        }
    }

    /// The command its advice is to run, without the `:`, which the command
    /// line offers next.
    pub fn suggestion(&self) -> Option<String> {
        match &self.advice {
            Some(Advice::Terminal(command) | Advice::Git(command)) => Some(command.clone()),
            None => None,
        }
    }

    /// Scrolls with the usual keys. Any other key closes the popup, which
    /// this returns false for.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.scroll.handle_key(key)
    }

    /// The lines to show: the output, and after it any advice.
    fn shown_lines(&self) -> Vec<Line<'static>> {
        let Some(advice) = &self.advice else {
            return self.lines.iter().map(|line| ansi_line(line)).collect();
        };
        // Dimmed, its own colours too, to set it apart from the advice.
        let dim = Style::default().fg(ui::DIM);
        let mut lines: Vec<Line> = self
            .lines
            .iter()
            .map(|line| Line::styled(strip_ansi(line), dim))
            .collect();
        let (lead, command) = match advice {
            Advice::Terminal(command) => ("It needs a terminal: run it as ", command),
            Advice::Git(command) => ("Git suggests ", command),
        };
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::raw(lead),
            Span::styled(format!(":{command}"), Style::default().fg(ui::KEY)),
            Span::styled("  (: then tab)", dim),
        ]));
        lines
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let lines = self.shown_lines();
        let widest = lines.iter().map(Line::width).max().unwrap_or(0);
        let width = (widest.max(self.title.chars().count() + 4) as u16 + 2)
            .clamp(40, area.width.saturating_sub(4).max(1));
        let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
        let popup = ui::centered(area, width, height);
        let mut block = ui::panel(self.title.clone(), true);
        if self.failed {
            block = block.border_style(Style::default().fg(Color::Red));
        }
        ui::text_popup(frame, popup, block, lines, &mut self.scroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advice_to_use_the_terminal_follows_the_dimmed_error() {
        let lines = vec!["error: There was a problem with the editor 'exit 1 #'.".to_string()];
        let popup = OutputPopup::new(
            "git commit",
            Finished {
                status: None,
                lines,
            },
        );
        let shown = popup.shown_lines();
        assert_eq!(shown[0].style.fg, Some(ui::DIM));
        let advice: String = shown[2]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(
            advice,
            "It needs a terminal: run it as :!git commit  (: then tab)"
        );
        assert_eq!(popup.suggestion().as_deref(), Some("!git commit"));
    }

    #[test]
    fn git_s_own_suggestions_are_offered_when_they_can_run_as_they_are() {
        let push = [
            "fatal: The current branch stashes has no upstream branch.",
            "To push the current branch and set the remote as upstream, use",
            "",
            "    git push --set-upstream origin stashes",
        ];
        let lines: Vec<String> = push.iter().map(|line| line.to_string()).collect();
        assert_eq!(
            suggested_command(&lines).as_deref(),
            Some("git push --set-upstream origin stashes")
        );
        let placeholder = vec!["    git branch --set-upstream-to=origin/<branch> main".to_string()];
        assert_eq!(suggested_command(&placeholder), None);
        // Mentioned in passing, not on a line of its own.
        let prose = vec!["hint: use git pull to integrate them".to_string()];
        assert_eq!(suggested_command(&prose), None);
        let popup = OutputPopup::new(
            "git push",
            Finished {
                status: None,
                lines,
            },
        );
        assert_eq!(
            popup.suggestion().as_deref(),
            Some("git push --set-upstream origin stashes")
        );
    }

    #[test]
    fn a_missing_editor_or_password_is_recognised() {
        let editor = ["error: There was a problem with the editor 'exit 1'.".to_string()];
        assert!(needed_terminal(&editor));
        let prompt = [
            "fatal: could not read Username for 'https://x': terminal prompts disabled".to_string(),
        ];
        assert!(needed_terminal(&prompt));
        assert!(!needed_terminal(&[
            "fatal: not a git repository".to_string()
        ]));
    }

    fn wait(running: &Running) -> Finished {
        let start = std::time::Instant::now();
        loop {
            if let Some(finished) = running.finished() {
                return finished;
            }
            assert!(start.elapsed().as_secs() < 5, "it didn't end");
            thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn stopping_a_command_stops_what_it_started() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30; echo late"]);
        let mut running = Running::start("sleepy".to_string(), command).unwrap();
        thread::sleep(std::time::Duration::from_millis(100));
        running.stop();
        let finished = wait(&running);
        assert!(!finished.succeeded() && finished.lines.is_empty());
    }

    #[test]
    fn a_command_runs_in_the_background_and_its_output_comes_back() {
        let mut command = Command::new("git");
        command.arg("--version");
        let running = Running::start("git --version".to_string(), command).unwrap();
        let finished = wait(&running);
        assert!(finished.succeeded());
        assert!(finished.lines[0].starts_with("git version"));
    }
}
