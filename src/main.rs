mod accent;
mod ansi;
mod app;
mod args;
mod branches;
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
mod review;
mod running;
mod staging;
mod terminal;
mod theme;
mod ui;
mod worker;

use anyhow::Result;
use app::App;
use picker::{Outcome, Picker};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let start = std::env::current_dir()?;
    let repo_root = git::repo_root().ok().map(PathBuf::from);
    match std::env::args().nth(1).as_deref() {
        Some("--color") => print_color(repo_root),
        // For the Neovim plugin's health check, to tell whether gclic and the
        // plugin come from the same version.
        Some("--version" | "-V") => {
            println!("gclic {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {}
    }
    let search_root = repos::search_root(&start, repo_root.as_deref());
    // Outside a repo, gclic offers the ones under where it started.
    if repo_root.is_none() && repos::find(&search_root).is_empty() {
        eprintln!("gclic: not in a git repo, and there are none under {}", repos::tilde(&start));
        std::process::exit(1);
    }

    let mut terminal = ratatui::init();
    terminal::enhance_keys()?;
    let result = run(&mut terminal, repo_root, search_root);
    terminal::plain_keys();
    ratatui::restore();
    result
}

/// `gclic --color`, for the Neovim plugin, or anything else that wants to
/// show a repo in the colour gclic gives it: the colour, as `#rrggbb`, or an
/// empty line if the colour bar is turned off, then the repo's folder. Fails
/// outside a repo.
fn print_color(repo_root: Option<PathBuf>) -> ! {
    let Some(root) = repo_root else {
        eprintln!("gclic: not in a git repo");
        std::process::exit(1);
    };
    // The colour comes from the repo's config and name, read from its root.
    if std::env::set_current_dir(&root).is_err() {
        std::process::exit(1);
    }
    let color = if accent::bar_shown() { accent::Accent::current().hex() } else { String::new() };
    println!("{color}\n{}", root.display());
    std::process::exit(0);
}

/// Opens the repo gclic is in, or the one picked if it isn't in one, and runs
/// the app until it quits.
fn run(terminal: &mut DefaultTerminal, repo_root: Option<PathBuf>, search_root: PathBuf) -> Result<()> {
    let (repo, search_root) = match repo_root {
        Some(root) => (root, search_root),
        None => match choose_repo(terminal, search_root)? {
            Some(found) => found,
            None => return Ok(()),
        },
    };
    // Paths from `git status` are relative to the repo root, so run from there.
    std::env::set_current_dir(repo)?;
    App::new(search_root).run(terminal)
}

/// Picks the repo to open, for when gclic starts outside one: the same list
/// as `:cd`, on its own. Returns the repo, and the search folder, which `..`
/// may have moved. None if the user leaves without picking.
fn choose_repo(terminal: &mut DefaultTerminal, mut root: PathBuf) -> Result<Option<(PathBuf, PathBuf)>> {
    let finder = |root: &Path| {
        let repos = repos::entries(root);
        let labels = repos.iter().map(|(label, _)| label.clone()).collect();
        let title = format!("Open a repo, in {}", repos::tilde(root));
        (Picker::single(&title, labels).with_hints(&repos::PICKER_KEYS), repos)
    };
    let (mut picker, mut repos) = finder(&root);
    loop {
        terminal.draw(|frame| picker.render(frame, frame.area()))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }
        match picker.handle_key(key) {
            Outcome::Open => {}
            Outcome::Cancelled => return Ok(None),
            Outcome::Picked(picked) => {
                let picked = picked.first();
                match repos.iter().find(|(label, _)| Some(label) == picked) {
                    Some((label, parent)) if label == repos::UP => {
                        root = parent.clone();
                        (picker, repos) = finder(&root);
                    }
                    Some((_, repo)) => return Ok(Some((repo.clone(), root))),
                    None => {}
                }
            }
        }
    }
}
