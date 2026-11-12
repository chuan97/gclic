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

/// A command's output as lines for the screen, still holding its colour
/// codes. Progress that git redraws in place, with carriage returns, keeps
/// only its last state.
pub fn clean_output(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let has_text = |part: &str| !strip_ansi(part).trim().is_empty();
    let lines = text.lines().map(|line| {
        let line = line.trim_end_matches('\r');
        let last = line.rsplit('\r').find(|part| has_text(part)).unwrap_or("");
        last.replace('\t', "    ")
    });
    let mut lines: Vec<String> = lines.collect();
    while lines.last().is_some_and(|line| !has_text(line)) {
        lines.pop();
    }
    lines
}

/// The pieces of a line: text, and the escape codes between it. Codes are
/// `ESC [ parameters final`, and only those ending in `m` set the style.
enum Piece<'a> {
    Text(&'a str),
    Style(&'a str),
    /// Any other code, such as erasing the rest of the line, which a popup has
    /// no use for.
    Other,
}

fn pieces(line: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('\x1b') {
        if start > 0 {
            pieces.push(Piece::Text(&rest[..start]));
        }
        let code = &rest[start + 1..];
        let Some(params) = code.strip_prefix('[') else {
            // An escape that isn't a CSI code: it and its next character go.
            let skip = code.chars().next().map_or(0, char::len_utf8);
            rest = &code[skip..];
            pieces.push(Piece::Other);
            continue;
        };
        // The code ends at its first character from @ to ~.
        match params.find(|ch: char| ('@'..='~').contains(&ch)) {
            Some(end) => {
                let is_style = params[end..].starts_with('m');
                pieces.push(if is_style { Piece::Style(&params[..end]) } else { Piece::Other });
                rest = &params[end + 1..];
            }
            None => {
                rest = "";
                pieces.push(Piece::Other);
            }
        }
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest));
    }
    pieces
}

/// A line's text without its escape codes.
pub fn strip_ansi(line: &str) -> String {
    pieces(line).into_iter().filter_map(|piece| if let Piece::Text(text) = piece { Some(text) } else { None }).collect()
}

/// A line with its colour codes turned into styles.
pub fn ansi_line(line: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    for piece in pieces(line) {
        match piece {
            Piece::Text(text) => spans.push(Span::styled(text.to_string(), style)),
            Piece::Style(params) => style = apply_sgr(style, params),
            Piece::Other => {}
        }
    }
    Line::from(spans)
}

/// The style after a "select graphic rendition" code's parameters, such as
/// `1;31` for bold red. The common ones: resets, bold, dim, italic,
/// underline, reverse, the 16 colours, and 256 and true colours.
fn apply_sgr(mut style: Style, params: &str) -> Style {
    use ratatui::style::Modifier;
    let numbers: Vec<u16> = params.split(';').map(|n| n.parse().unwrap_or(0)).collect();
    let basic = |n: u16| {
        [Color::Black, Color::Red, Color::Green, Color::Yellow, Color::Blue, Color::Magenta, Color::Cyan, Color::Gray]
            [n as usize]
    };
    let bright = |n: u16| {
        [
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ][n as usize]
    };
    let mut i = 0;
    while i < numbers.len() {
        let n = numbers[i];
        // 38 and 48 take a colour from the parameters after them.
        let mut extended = || match numbers.get(i + 1) {
            Some(5) => {
                let color = numbers.get(i + 2).map(|&c| Color::Indexed(c as u8));
                i += 2;
                color
            }
            Some(2) => {
                let channel = |k: usize| numbers.get(i + k).map(|&c| c as u8);
                let color = match (channel(2), channel(3), channel(4)) {
                    (Some(r), Some(g), Some(b)) => Some(Color::Rgb(r, g, b)),
                    _ => None,
                };
                i += 4;
                color
            }
            _ => None,
        };
        style = match n {
            0 => Style::default(),
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            7 => style.add_modifier(Modifier::REVERSED),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            27 => style.remove_modifier(Modifier::REVERSED),
            30..=37 => style.fg(basic(n - 30)),
            38 => extended().map_or(style, |color| style.fg(color)),
            39 => style.fg(Color::Reset),
            40..=47 => style.bg(basic(n - 40)),
            48 => extended().map_or(style, |color| style.bg(color)),
            49 => style.bg(Color::Reset),
            90..=97 => style.fg(bright(n - 90)),
            100..=107 => style.bg(bright(n - 100)),
            _ => style,
        };
        i += 1;
    }
    style
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
    lines.iter().map(|line| strip_ansi(line)).any(|line| SIGNS.iter().any(|sign| line.contains(sign)))
}

/// A command git suggests in its output, to run as it is: one on a line of its
/// own, indented, as in "To push the current branch … use" followed by
/// `    git push --set-upstream origin main`. Ones with placeholders, such as
/// `<branch>`, are left out, as they'd need filling in.
pub fn suggested_command(lines: &[String]) -> Option<String> {
    lines.iter().map(|line| strip_ansi(line)).find_map(|line| {
        let indented = line.starts_with(char::is_whitespace);
        let command = line.trim();
        let runnable = command.starts_with("git ") && !command.contains(['<', '>']) && !command.contains("...");
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
/// A command to run next, after one that failed, as typed after `:`.
enum Advice {
    /// The same command, in the terminal, as it asked for something.
    Terminal(String),
    /// What git suggested.
    Git(String),
}

pub struct OutputPopup {
    title: String,
    lines: Vec<String>,
    failed: bool,
    /// gclic's own advice, after the command's output, which is then dimmed
    /// to set the two apart.
    advice: Option<Advice>,
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
        let advice = if !failed {
            None
        } else if needed_terminal(&lines) {
            Some(Advice::Terminal(format!("!{label}")))
        } else {
            suggested_command(&lines).map(Advice::Git)
        };
        OutputPopup { title: format!(" {label} "), lines, failed, advice, scroll: 0, page: 0, max_scroll: 0 }
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
        let Some(advice) = &self.advice else {
            return self.lines.iter().map(|line| ansi_line(line)).collect();
        };
        // Dimmed, its own colours too, to set it apart from the advice.
        let dim = Style::default().fg(ui::DIM);
        let mut lines: Vec<Line> = self.lines.iter().map(|line| Line::styled(strip_ansi(line), dim)).collect();
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
    fn colour_codes_become_styles() {
        let line = ansi_line("\x1b[33mabc1234\x1b[m \x1b[1;32mmain\x1b[0m done");
        let pieces: Vec<_> = line.spans.iter().map(|span| (span.content.as_ref(), span.style)).collect();
        assert_eq!(
            pieces,
            [
                ("abc1234", Style::default().fg(Color::Yellow)),
                (" ", Style::default()),
                ("main", Style::default().fg(Color::Green).add_modifier(ratatui::style::Modifier::BOLD)),
                (" done", Style::default()),
            ]
        );
    }

    #[test]
    fn extended_colours_and_other_codes() {
        let line = ansi_line("\x1b[38;5;208mo\x1b[48;2;1;2;3mx\x1b[Kz");
        assert_eq!(line.spans[0].style.fg, Some(Color::Indexed(208)));
        assert_eq!(line.spans[1].style.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(strip_ansi("remote: done\x1b[K"), "remote: done");
        assert_eq!(strip_ansi("\x1b[1mcut\x1b["), "cut");
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
        assert_eq!(advice, "It needs a terminal: run it as :!git commit  (: then tab)");
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
        assert_eq!(suggested_command(&lines).as_deref(), Some("git push --set-upstream origin stashes"));
        let placeholder = vec!["    git branch --set-upstream-to=origin/<branch> main".to_string()];
        assert_eq!(suggested_command(&placeholder), None);
        // Mentioned in passing, not on a line of its own.
        let prose = vec!["hint: use git pull to integrate them".to_string()];
        assert_eq!(suggested_command(&prose), None);
        let popup = OutputPopup::new("git push", Finished { status: None, lines });
        assert_eq!(popup.suggestion().as_deref(), Some("git push --set-upstream origin stashes"));
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
