mod accent;
mod app;
mod branches;
mod commands;
mod compare;
mod details;
mod diff;
mod editor;
mod file_list;
mod git;
mod graph;
mod help;
mod highlight;
mod picker;
mod repos;
mod staging;
mod ui;
mod worker;

use anyhow::Result;
use app::App;
use ratatui::DefaultTerminal;
use std::path::PathBuf;

fn main() -> Result<()> {
    let start = std::env::current_dir()?;
    let repo_root = git::repo_root().ok().map(PathBuf::from);
    let search_root = repos::search_root(&start, repo_root.as_deref());
    // Outside a repo, gclic offers the ones under where it started.
    if repo_root.is_none() && repos::find(&search_root).is_empty() {
        eprintln!("gclic: not in a git repo, and there are none under {}", repos::tilde(&start));
        std::process::exit(1);
    }

    let mut terminal = ratatui::init();
    commands::enhance_keys()?;
    let result = run(&mut terminal, repo_root, search_root);
    commands::plain_keys();
    ratatui::restore();
    result
}

/// Opens the repo gclic is in, or the one picked if it isn't in one, and runs
/// the app until it quits.
fn run(terminal: &mut DefaultTerminal, repo_root: Option<PathBuf>, search_root: PathBuf) -> Result<()> {
    let (repo, search_root) = match repo_root {
        Some(root) => (root, search_root),
        None => match app::choose_repo(terminal, search_root)? {
            Some(found) => found,
            None => return Ok(()),
        },
    };
    // Paths from `git status` are relative to the repo root, so run from there.
    std::env::set_current_dir(repo)?;
    App::new(search_root).run(terminal)
}
