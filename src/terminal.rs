//! Handing the terminal to a command, as `:!` and terminal editors need,
//! while gclic steps aside, and the keyboard flags it sets on the terminal.

use anyhow::Result;
use ratatui::crossterm::cursor::{MoveTo, Show, position};
use ratatui::crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    Clear as ClearScreen, ClearType, EnterAlternateScreen, enable_raw_mode, supports_keyboard_enhancement,
};
use ratatui::DefaultTerminal;
use std::io::BufRead;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

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
    hand_terminal(terminal, line, &format!("!{line}"), shell(line))
}

/// Gives the terminal to a command, as `run_in_terminal` does, showing what
/// runs as `shown`, and naming it `label` in a note. The first of `commands`
/// that exists runs, as a shell may not be installed under its first name.
pub fn hand_terminal(
    terminal: &mut DefaultTerminal,
    shown: &str,
    label: &str,
    commands: Vec<Command>,
) -> Result<Option<String>> {
    plain_keys();
    ratatui::restore();
    let _ = execute!(std::io::stdout(), Show, ClearScreen(ClearType::All), MoveTo(0, 0));
    println!("$ {shown}");
    let before = position().ok();
    let mut result = Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    for mut command in commands {
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
        outcome.map(|outcome| format!("{label}: {}", outcome.to_lowercase()))
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
