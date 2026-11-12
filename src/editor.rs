//! Opening a file in the user's editor, at the line they're on.
//!
//! The editor is `$GCLIC_EDITOR`, `$VISUAL` or `$EDITOR`, the first one set,
//! as git picks its own. gclic knows how the common editors take a line, and a
//! setting with `{file}` and `{line}` in it says so for any other. GUI editors
//! start in the background. Terminal editors get the terminal, as `:!`
//! commands do, and gclic comes back when they quit. Inside Neovim, where
//! `$NVIM` says how to reach it, the file opens in that Neovim instead.

use crate::commands::split_args;
use std::path::Path;
use std::process::Command;

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
pub fn parent_nvim() -> Option<String> {
    std::env::var("NVIM").ok().filter(|address| !address.is_empty())
}

/// Asks the Neovim at `address` to evaluate an expression, in the background.
fn nvim_expr(address: &str, expr: &str) -> Command {
    let mut command = Command::new("nvim");
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

/// Makes the parent Neovim's working folder the repo gclic switched to, so
/// its file finder, search and file tree follow.
pub fn cd_nvim(address: &str, folder: &Path) -> Command {
    let lua = "vim.cmd.cd(vim.fn.fnameescape(_A))";
    nvim_expr(address, &format!("luaeval({}, {})", vim_string(lua), vim_string(&folder.display().to_string())))
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
    fn vim_strings_double_their_quotes() {
        assert_eq!(vim_string("it's"), "'it''s'");
    }
}
