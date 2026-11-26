//! Opening a file in the user's editor, at the line they're on.
//!
//! The editor is `$GCLIC_EDITOR`, `$VISUAL` or `$EDITOR`, the first one set,
//! as git picks its own. gclic knows how the common editors take a line, and a
//! setting with `{file}` and `{line}` in it says so for any other. GUI editors
//! start in the background. Terminal editors get the terminal, as `:!`
//! commands do, and gclic comes back when they quit. Inside Neovim, where
//! `$NVIM` says how to reach it, the file opens in that Neovim instead.

use crate::args::split_args;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// How to open a file.
pub enum Launch {
    /// A GUI editor, which gclic starts and leaves running.
    Background(Command),
    /// A terminal editor, which gets the terminal until it quits. With what
    /// to show above it, as `:!` shows its command.
    Terminal(String, Command),
}

/// The editor setting in use: the first of these that's set.
pub fn setting() -> String {
    ["GCLIC_EDITOR", "VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| if cfg!(windows) { "notepad".to_string() } else { "vi".to_string() })
}

/// The setting's words, split as a shell would, unless the whole of it is
/// the path of a program: Windows paths often have spaces, and settings with
/// them often lack quotes.
fn words(setting: &str) -> Result<Vec<String>, String> {
    if Path::new(setting.trim()).is_file() {
        return Ok(vec![setting.trim().to_string()]);
    }
    split_args(setting)
}

/// The editor's name, for the hint: its program, without folders or `.exe`.
pub fn name(setting: &str) -> String {
    let program = words(setting).ok().and_then(|args| args.into_iter().next()).unwrap_or_default();
    program_name(&program)
}

fn program_name(program: &str) -> String {
    let file = Path::new(program).file_name().map_or(program.into(), |name| name.to_string_lossy());
    file.trim_end_matches(".exe").to_lowercase()
}

/// How the editors gclic knows take a line.
enum LineArgs {
    /// `+42 file`: vi and the editors that follow it.
    Plus,
    /// `file:42`.
    Colon,
    /// `--goto file:42`: VS Code and the editors built on it.
    Goto,
    /// Just the file.
    None,
}

fn line_args(program: &str) -> LineArgs {
    match program {
        "vi" | "vim" | "nvim" | "gvim" | "nano" | "micro" | "emacs" | "emacsclient" | "kak" | "ne" | "mg" => {
            LineArgs::Plus
        }
        "hx" | "helix" | "zed" | "subl" => LineArgs::Colon,
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => LineArgs::Goto,
        _ => LineArgs::None,
    }
}

/// Editors with windows of their own, which gclic leaves running in the
/// background rather than step aside for.
fn is_gui(program: &str) -> bool {
    matches!(
        program,
        "zed" | "subl" | "code" | "code-insiders" | "codium" | "cursor" | "windsurf" | "gvim" | "notepad"
    )
}

/// How to open `file` at `line` with this editor setting.
pub fn launch(setting: &str, file: &Path, line: usize) -> Result<Launch, String> {
    let args = words(setting)?;
    let Some((program, own_args)) = args.split_first() else {
        return Err("The editor setting is empty".to_string());
    };
    let file = file.display().to_string();
    let templated = args.iter().any(|arg| arg.contains("{file}"));
    let mut full: Vec<String> = Vec::new();
    if templated {
        full.extend(own_args.iter().map(|arg| arg.replace("{file}", &file).replace("{line}", &line.to_string())));
    } else {
        full.extend(own_args.iter().cloned());
        match line_args(&program_name(program)) {
            LineArgs::Plus => full.extend([format!("+{line}"), file]),
            LineArgs::Colon => full.push(format!("{file}:{line}")),
            LineArgs::Goto => full.extend(["--goto".to_string(), format!("{file}:{line}")]),
            LineArgs::None => full.push(file),
        }
    }
    let mut command = Command::new(program);
    command.args(&full);
    Ok(if is_gui(&program_name(program)) {
        Launch::Background(command)
    } else {
        Launch::Terminal(format!("{program} {}", full.join(" ")), command)
    })
}

/// The Neovim gclic runs inside, if it does: `$NVIM`, where it listens.
/// Never in tests, which may be run in Neovim's terminal, and would move it
/// to their repos.
pub fn parent_nvim() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    std::env::var("NVIM").ok().filter(|address| !address.is_empty())
}

/// Asks the Neovim at `address` to evaluate an expression, in the background.
/// Through the very Neovim gclic runs in, which the plugin names in
/// `$GCLIC_NVIM`, as `nvim` on the `PATH` may be another, or none.
fn nvim_expr(address: &str, expr: &str) -> Command {
    let program = std::env::var("GCLIC_NVIM").ok().filter(|program| !program.is_empty());
    let mut command = Command::new(program.as_deref().unwrap_or("nvim"));
    command.args(["--server", address, "--remote-expr", expr]);
    command
}

/// A string as Vim script writes it: in single quotes, which double.
fn vim_string(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Opens a file at a line in the parent Neovim. Through gclic's Neovim plugin
/// when it's there, which hides gclic's window and opens the file in the
/// window below it, or else in a new tab, as a terminal window opened some
/// other way is no place to open it.
pub fn open_in_nvim(address: &str, file: &Path, line: usize) -> Command {
    let lua = "(function(a) \
               local ok, gclic = pcall(require, \"gclic\") \
               if ok and gclic.open then gclic.open(a[1], a[2]) return end \
               vim.cmd(\"tab drop \" .. vim.fn.fnameescape(a[1])) \
               pcall(vim.api.nvim_win_set_cursor, 0, { a[2], 0 }) \
               end)(_A)";
    let file = vim_string(&file.display().to_string());
    nvim_expr(address, &format!("luaeval({}, [{file}, {line}])", vim_string(lua)))
}

/// Asks the parent Neovim to show the repo's colour again, after `:color`
/// changed it. Nothing happens without gclic's plugin there.
pub fn refresh_nvim_accent(address: &str) -> Command {
    let lua = "pcall(function() require(\"gclic\").refresh_accent() end)";
    nvim_expr(address, &format!("luaeval({})", vim_string(lua)))
}

/// Makes the parent Neovim's working folder the repo gclic switched to, so
/// its file finder, search and file tree follow.
pub fn cd_nvim(address: &str, folder: &Path) -> Command {
    let lua = "vim.cmd.cd(vim.fn.fnameescape(_A))";
    nvim_expr(address, &format!("luaeval({}, {})", vim_string(lua), vim_string(&folder.display().to_string())))
}

/// Following the Neovim gclic runs in: its plugin writes Neovim's working
/// folder to a file, named in `$GCLIC_FOLLOW`, whenever it changes, and gclic
/// switches to the repo that folder is in. A file rather than keys typed into
/// gclic, which could land in a popup or a search.
pub struct Follow {
    file: PathBuf,
    /// When the file last changed, as far as gclic has seen.
    seen: Option<SystemTime>,
}

impl Follow {
    /// Inside Neovim, with gclic's plugin there to write the file.
    /// Never in tests, which may be run in Neovim's terminal.
    pub fn from_env() -> Option<Follow> {
        if cfg!(test) {
            return None;
        }
        let file = std::env::var_os("GCLIC_FOLLOW").filter(|file| !file.is_empty())?;
        Some(Follow { file: PathBuf::from(file), seen: None })
    }

    /// Following the folders written to `file`.
    #[cfg(test)]
    pub fn at(file: &Path) -> Follow {
        Follow { file: file.to_path_buf(), seen: None }
    }

    /// The root of the repo Neovim has moved to, if it wrote a folder since
    /// last asked and that folder is in a repo other than `current`. Folders
    /// outside a repo are passed over.
    pub fn moved_to(&mut self, current: &Path) -> Option<PathBuf> {
        let changed = std::fs::metadata(&self.file).and_then(|meta| meta.modified()).ok()?;
        if self.seen == Some(changed) {
            return None;
        }
        self.seen = Some(changed);
        let folder = std::fs::read_to_string(&self.file).ok()?;
        let root = crate::git::Repo::discover(Path::new(folder.trim()))?.root().to_path_buf();
        (!crate::repos::same_folder(&root, current)).then_some(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The program and arguments a launch would run, and whether in the terminal.
    fn run(setting: &str) -> (Vec<String>, bool) {
        let (command, terminal) = match launch(setting, Path::new("/repo/src/main.rs"), 42).unwrap() {
            Launch::Background(command) => (command, false),
            Launch::Terminal(_, command) => (command, true),
        };
        let mut words = vec![command.get_program().to_string_lossy().into_owned()];
        words.extend(command.get_args().map(|arg| arg.to_string_lossy().into_owned()));
        (words, terminal)
    }

    #[test]
    fn each_editor_gets_the_line_its_own_way() {
        assert_eq!(run("nvim"), (vec!["nvim".into(), "+42".into(), "/repo/src/main.rs".into()], true));
        assert_eq!(run("hx"), (vec!["hx".into(), "/repo/src/main.rs:42".into()], true));
        assert_eq!(run("zed"), (vec!["zed".into(), "/repo/src/main.rs:42".into()], false));
        let code = vec!["code".into(), "--goto".into(), "/repo/src/main.rs:42".into()];
        assert_eq!(run("code"), (code, false));
    }

    #[test]
    fn a_setting_keeps_its_own_arguments_and_its_program_path() {
        let emacs = vec!["emacs".into(), "-nw".into(), "+42".into(), "/repo/src/main.rs".into()];
        assert_eq!(run("emacs -nw"), (emacs, true));
        assert_eq!(run("/usr/local/bin/nvim").0[1], "+42");
        assert_eq!(name("/usr/local/bin/nvim"), "nvim");
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_path_names_its_program() {
        assert_eq!(name(r#""C:\Program Files\Neovim\bin\nvim.exe""#), "nvim");
    }

    #[test]
    fn a_program_s_whole_path_isnt_split_at_its_spaces() {
        let folder = std::env::temp_dir().join(format!("gclic editor {}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let program = folder.join("my editor");
        std::fs::write(&program, "").unwrap();
        let setting = program.display().to_string();
        assert_eq!(name(&setting), "my editor");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_template_says_where_the_file_and_line_go() {
        let custom = vec!["myeditor".into(), "--line".into(), "42".into(), "/repo/src/main.rs".into()];
        assert_eq!(run("myeditor --line {line} {file}"), (custom, true));
        // Unknown and without a template, it just gets the file.
        assert_eq!(run("myeditor").0, ["myeditor", "/repo/src/main.rs"]);
    }

    #[test]
    fn following_neovim_finds_the_repo_it_moved_to_once() {
        let base = std::env::temp_dir().join(format!("gclic-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (here, there) = (base.join("here"), base.join("there"));
        for repo in [&here, &there] {
            std::fs::create_dir_all(repo.join("src")).unwrap();
            Command::new("git").args(["init", "-q"]).current_dir(repo).status().unwrap();
        }
        let file = base.join("follow");
        let mut follow = Follow { file: file.clone(), seen: None };
        assert_eq!(follow.moved_to(&here), None); // no file yet
        // A subfolder of the repo gclic is in: nowhere to go.
        std::fs::write(&file, here.join("src").display().to_string()).unwrap();
        assert_eq!(follow.moved_to(&here), None);
        // Another repo, given by a subfolder of it: its root, once.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&file, there.join("src").display().to_string()).unwrap();
        let moved = follow.moved_to(&here).map(|root| root.canonicalize().unwrap());
        assert_eq!(moved, Some(there.canonicalize().unwrap()));
        assert_eq!(follow.moved_to(&here), None);
        // Outside any repo: passed over.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&file, std::env::temp_dir().display().to_string()).unwrap();
        assert_eq!(follow.moved_to(&here), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn vim_strings_double_their_quotes() {
        assert_eq!(vim_string("it's"), "'it''s'");
    }
}
