mod app;
mod branches;
mod compare;
mod details;
mod diff;
mod file_list;
mod git;
mod graph;
mod help;
mod highlight;
mod picker;
mod staging;
mod ui;
mod worker;

use anyhow::Result;
use app::App;
use ratatui::crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;

fn main() -> Result<()> {
    let root = match git::repo_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("gclic: {error}");
            std::process::exit(1);
        }
    };
    // Paths from `git status` are relative to the repo root, so run from there.
    std::env::set_current_dir(root)?;

    let app = App::new();
    let mut terminal = ratatui::init();
    // Most terminals send ctrl-enter as plain enter, so gclic asks those that
    // can to report it apart, for committing. Windows' console always does.
    // Elsewhere ctrl-s commits too, as the help says.
    let enhanced = matches!(supports_keyboard_enhancement(), Ok(true));
    if enhanced {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        execute!(std::io::stdout(), PushKeyboardEnhancementFlags(flags))?;
    }
    let result = app.run(&mut terminal);
    if enhanced {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    result
}
