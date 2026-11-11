//! Commands the user types on the command line: `:git …`, run in the
//! background with its output captured, and `:!…`, run in the terminal
//! itself while gclic steps aside, for anything that needs it.

use crate::ui;
use anyhow::Result;
use ratatui::crossterm::cursor::{MoveTo, Show, position};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    Clear as ClearScreen, ClearType, EnterAlternateScreen, enable_raw_mode, supports_keyboard_enhancement,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;
use ratatui::{DefaultTerminal, Frame};
use std::io::{BufRead, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

/// Splits a command line into arguments, as a shell would for simple cases:
/// spaces separate them, and quotes keep spaces in. Inside double quotes, and
/// outside quotes, a backslash only escapes a quote, a space or another
/// backslash, so Windows paths keep theirs.
pub fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    // Whether an argument has started, so `""` makes an empty one.
    let mut started = false;
    let mut quote = None;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => current.push(ch),
            (_, '\\') if matches!(chars.peek(), Some(next) if next.is_whitespace() || matches!(next, '\'' | '"' | '\\')) => {
                current.extend(chars.next());
                started = true;
            }
            (Some(_), _) => current.push(ch),
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, _) if ch.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, _) => {
                current.push(ch);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("A quote isn't closed".to_string());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

/// A command's output as lines for the screen. Progress that git redraws in
/// place, with carriage returns, keeps only its last state.
pub fn clean_output(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let lines = text.lines().map(|line| {
        let line = line.trim_end_matches('\r');
        let last = line.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or("");
        last.replace('\t', "    ")
    });
    let mut lines: Vec<String> = lines.collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// Whether a command failed for want of a terminal: for an editor, a
/// password, or a passphrase, which `:git` turns off.
pub fn needed_terminal(lines: &[String]) -> bool {
    const SIGNS: [&str; 4] = [
        "problem with the editor",
        "terminal prompts disabled",
        "could not read Username",
        "could not read Password",
    ];
    lines.iter().any(|line| SIGNS.iter().any(|sign| line.contains(sign)))
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
        let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
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
            let _ = send.send(Finished { status, lines: clean_output(&bytes) });
        });
        Ok(Running { label, child, finished, stopped: false })
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
            Command::new("taskkill").args(["/T", "/F", "/PID", &pid]).output()
        } else {
            // The group has the command's id, as it leads it.
            Command::new("kill").args(["-TERM", "--", &format!("-{pid}")]).output()
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

/// Whether the terminal can report keys apart, asked once: asking means
/// waiting for its answer, and one that doesn't answer makes gclic wait.
static CAN_ENHANCE: OnceLock<bool> = OnceLock::new();
/// Whether the enhanced keys are on now, to turn them off for `:!`.
static ENHANCED_KEYS: AtomicBool = AtomicBool::new(false);

/// Most terminals send ctrl-enter as plain enter, so gclic asks those that can
/// to report it apart, for committing. Windows' console always does.
/// Elsewhere ctrl-s commits too, as the help says.
pub fn enhance_keys() -> Result<()> {
    if *CAN_ENHANCE.get_or_init(|| matches!(supports_keyboard_enhancement(), Ok(true))) {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        execute!(std::io::stdout(), PushKeyboardEnhancementFlags(flags))?;
        ENHANCED_KEYS.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Back to the keys the terminal sends by default, for other programs.
pub fn plain_keys() {
    if ENHANCED_KEYS.swap(false, Ordering::Relaxed) {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
}

/// The shell `:!` runs a line in: the user's own on Unix, PowerShell on
/// Windows.
fn shell(line: &str) -> Vec<Command> {
    if cfg!(windows) {
        // PowerShell 7 if it's installed, or the one Windows comes with.
        ["pwsh", "powershell"]
            .into_iter()
            .map(|program| {
                let mut command = Command::new(program);
                command.args(["-NoLogo", "-Command", line]);
                command
            })
            .collect()
    } else {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        let mut command = Command::new(program);
        command.args(["-c", line]);
        vec![command]
    }
}

/// Runs a line in the shell, in the terminal itself: gclic leaves the screen
/// as it was before it started, and the command has the terminal to itself.
///
/// If it printed anything, gclic waits for enter, so it can be read. If not,
/// as with a full-screen program such as vim, which restores the screen when
/// it quits, gclic comes straight back. The cursor tells them apart: printing
/// moves it. The screen is cleared first, so that printing can't scroll the
/// cursor back to where it was. Returns a note for the message line, when
/// gclic came straight back from a command that failed.
pub fn run_in_terminal(terminal: &mut DefaultTerminal, line: &str) -> Result<Option<String>> {
    plain_keys();
    ratatui::restore();
    let _ = execute!(std::io::stdout(), Show, ClearScreen(ClearType::All), MoveTo(0, 0));
    println!("$ {line}");
    let before = position().ok();
    let mut result = Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    for mut command in shell(line) {
        result = command.spawn().and_then(|mut child| {
            let _leave_interrupts = LeaveInterrupts::start();
            child.wait()
        });
        if !matches!(&result, Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
            break;
        }
    }
    let printed = before.is_none() || position().ok() != before;
    let outcome = match &result {
        Ok(status) if status.success() => None,
        Ok(status) => Some(match status.code() {
            Some(code) => format!("It exited with {code}"),
            None => "It was stopped".to_string(),
        }),
        Err(error) => Some(format!("Couldn't run it: {error}")),
    };
    let note = if printed {
        let outcome = outcome.as_deref().unwrap_or("Done");
        println!("\n[gclic] {outcome}. Press enter to go back");
        let _ = std::io::stdin().lock().read_line(&mut String::new());
        None
    } else {
        outcome.map(|outcome| format!("!{line}: {}", outcome.to_lowercase()))
    };

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    enhance_keys()?;
    // The screen was the shell's, so everything is drawn again.
    terminal.clear()?;
    Ok(note)
}

/// While a command runs in the terminal, ctrl-c and ctrl-\ are for it: the
/// terminal sends them to every program in the foreground, gclic included,
/// which they would otherwise stop. Started after the command is, as programs
/// inherit ignoring them, and the command must still stop on them.
struct LeaveInterrupts;

impl LeaveInterrupts {
    fn start() -> LeaveInterrupts {
        // SAFETY: setting signals to be ignored, then back to their default
        // below, with no handler of gclic's own involved.
        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        }
        // SAFETY: with no handler given, this only makes the console's ctrl-c
        // pass gclic by, until it's undone below.
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 1);
        }
        LeaveInterrupts
    }
}

impl Drop for LeaveInterrupts {
    fn drop(&mut self) {
        // SAFETY: as above.
        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
        }
        // SAFETY: as above.
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0);
        }
    }
}

/// What a `:git` command printed, in a popup to scroll through. Red when the
/// command failed.
pub struct OutputPopup {
    title: String,
    lines: Vec<String>,
    failed: bool,
    /// gclic's own advice, after the command's output, which is then dimmed
    /// to set the two apart: the command to run instead, in the terminal.
    instead: Option<String>,
    scroll: usize,
    page: usize,
    max_scroll: usize,
}

impl OutputPopup {
    pub fn new(label: &str, finished: Finished) -> OutputPopup {
        let failed = !finished.succeeded();
        let mut lines = finished.lines;
        if lines.is_empty() {
            lines.push("(no output)".to_string());
        }
        let instead = (failed && needed_terminal(&lines)).then(|| format!(":!{label}"));
        OutputPopup { title: format!(" {label} "), lines, failed, instead, scroll: 0, page: 0, max_scroll: 0 }
    }

    /// Scrolls with the usual keys. Any other key closes the popup, which
    /// this returns false for.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let half_page = (self.page / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll = (self.scroll + 1).min(self.max_scroll),
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.scroll = (self.scroll + half_page).min(self.max_scroll),
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(half_page),
            KeyCode::Char('g') => self.scroll = 0,
            KeyCode::Char('G') => self.scroll = self.max_scroll,
            _ => return false,
        }
        true
    }

    /// The lines to show: the output, and after it any advice.
    fn shown_lines(&self) -> Vec<Line<'static>> {
        let Some(instead) = &self.instead else {
            return self.lines.iter().map(|line| Line::raw(line.clone())).collect();
        };
        let dim = Style::default().fg(ui::DIM);
        let mut lines: Vec<Line> = self.lines.iter().map(|line| Line::styled(line.clone(), dim)).collect();
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::raw("It needs a terminal: run it as "),
            Span::styled(instead.clone(), Style::default().fg(ui::KEY)),
        ]));
        lines
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let lines = self.shown_lines();
        let widest = lines.iter().map(Line::width).max().unwrap_or(0);
        let width = (widest.max(self.title.chars().count() + 4) as u16 + 2).clamp(40, area.width.saturating_sub(4).max(1));
        let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
        self.page = height.saturating_sub(2) as usize;
        self.max_scroll = lines.len().saturating_sub(self.page);
        self.scroll = self.scroll.min(self.max_scroll);
        let closing = if self.max_scroll > 0 {
            ui::key_hints(&[("j/k", "scroll"), ("any other key", "closes")], None)
        } else {
            ui::key_hints(&[("any key", "closes")], None)
        };

        let popup = ui::centered(area, width, height);
        frame.render_widget(Clear, popup);
        let mut block = ui::panel(self.title.clone(), true).title_bottom(closing);
        if self.failed {
            block = block.border_style(Style::default().fg(Color::Red));
        }
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        ui::render_lines(frame, inner, lines.into_iter().skip(self.scroll).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(line: &str) -> Vec<String> {
        split_args(line).unwrap()
    }

    #[test]
    fn arguments_split_on_spaces_and_keep_quoted_ones_whole() {
        assert_eq!(split("commit -m \"fix the  bug\""), ["commit", "-m", "fix the  bug"]);
        assert_eq!(split("log  --format='%h %s'"), ["log", "--format=%h %s"]);
        assert_eq!(split("commit -m \"\""), ["commit", "-m", ""]);
        assert_eq!(split(r#"commit -m "say \"hi\"""#), ["commit", "-m", "say \"hi\""]);
        assert_eq!(split(r"add my\ file"), ["add", "my file"]);
        assert!(split_args("commit -m \"open").is_err());
    }

    #[test]
    fn windows_paths_keep_their_backslashes() {
        assert_eq!(split(r"add src\main.rs"), ["add", r"src\main.rs"]);
    }

    #[test]
    fn redrawn_progress_keeps_its_last_state() {
        let output = b"Receiving objects:  50% (1/2)\rReceiving objects: 100% (2/2), done.\r\nDone\n\n";
        assert_eq!(clean_output(output), ["Receiving objects: 100% (2/2), done.", "Done"]);
    }

    #[test]
    fn advice_to_use_the_terminal_follows_the_dimmed_error() {
        let lines = vec!["error: There was a problem with the editor 'exit 1 #'.".to_string()];
        let popup = OutputPopup::new("git commit", Finished { status: None, lines });
        let shown = popup.shown_lines();
        assert_eq!(shown[0].style.fg, Some(ui::DIM));
        let advice: String = shown[2].spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(advice, "It needs a terminal: run it as :!git commit");
    }

    #[test]
    fn a_missing_editor_or_password_is_recognised() {
        let editor = ["error: There was a problem with the editor 'exit 1'.".to_string()];
        assert!(needed_terminal(&editor));
        let prompt = ["fatal: could not read Username for 'https://x': terminal prompts disabled".to_string()];
        assert!(needed_terminal(&prompt));
        assert!(!needed_terminal(&["fatal: not a git repository".to_string()]));
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
