mod app;
mod compare;
mod details;
mod diff;
mod file_list;
mod git;
mod graph;
mod picker;
mod staging;
mod ui;

use anyhow::Result;
use app::App;

fn main() -> Result<()> {
    let root = match git::repo_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("mygc: {error}");
            std::process::exit(1);
        }
    };
    // Paths from `git status` are relative to the repo root, so run from there.
    std::env::set_current_dir(root)?;

    let app = App::new();
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}
