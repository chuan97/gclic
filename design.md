# gclic: design notes

gclic stands for git cli companion. It was first called mygc.

## Original brief

**Goal:** build a TUI git client in Rust.

**Subgoal:** learn Rust in the process.

**Design constraint:** the codebase must stay simple and easy to follow. The author must grow with the project.

**Scope:** this is not a complete client. git's CLI is great for many use cases, but for some things a TUI saves a lot of time or does what the CLI simply cannot. Four features are in scope:

1. **Visual staging area and commit.** Browse changed files, stage or unstage them, restore unstaged files, and commit whatever is staged. Any file's diff can be opened in a split view. It should mimic VS Code's git panel staging area, with a TUI flavour and vim-style keyboard navigation.
2. **Branch viewer**, inspired by VS Code's Git Graph, below the staging area.
   - By default it shows the current branch and its remote version, e.g. `feature` and `origin/feature`.
   - Nice, light visuals rather than a cluttered log-like list: a commit chain where each commit is a bead and beads are joined by lines, with a change of color at the transition between local and origin.
   - Commits are navigable by keyboard. Opening one shows its changed files, and opening a file shows its diff.
   - A box with the commit message and metadata can be toggled, and the commit sha can be copied to the clipboard.
   - Any number of branches, local or remote, can be selected to render instead of the default two.
3. **Branch picker.** List existing branches, fuzzy search or scroll to select one, and check it out.
4. **Differ of two arbitrary refs.** Select two refs. The view shows how many commits A is ahead of and behind B since their common ancestor, and a navigable list of differing files. Opening one shows a split diff.

**Approach:** the features are fairly separate, so they can advance modularly, one at a time, starting with 1 and 2. Before going in seriously, one-shot the entire tool as an MVP, then use it to refine what we want and implement it for real, feature by feature.

## Where it stands

The MVP is feature complete, and on macOS it's a fully working app: every feature in the brief is in, along with what using it asked for. A rewrite from here would be a learning exercise and an architecture review, not a feature project. The notes below describe what was built, so they can serve as its starting point.

The approved MVP plan and the first round of feedback on it are in git history: see `design.md` as of commit `ecb4f54`.

## What it looks like

Two tabs, `Repo` and `Compare`, switched with `tab`. A hint bar at the bottom lists the keys that apply right now, with `? help · q quit` after them.

- **Repo:** a left column of three panes, Branches, Changes and Graph, moved between with `h`/`l`. Their heights follow focus: Branches shows only the checked-out branch until focused, Changes fits its files, and the graph takes the rest. The panes `h` and `l` lead to are marked `[h]` and `[l]` after their titles.
- **Compare:** how two refs differ, started with `c` from Branches, the graph or the tab itself.
- **The right side:** a preview that takes 80% of the width when open, either a diff or a commit's details, never both. Each stays in its mode: moving onto a commit leaves a diff alone, and moving onto a file leaves the details alone (a file shows its own commit's details). `enter` on a file shows its diff, a second `enter` moves into it, and `esc` comes back, then closes it. `z` zooms it to the full width.
- **Popups:** a title saying what they are, and their keys in the bottom border. Keys are yellow, and the key for a destructive action (discard, delete, close a comparison, quit) is bold red. Borders are cyan, except git errors, which are red.

## How it's built

### Modules

| File | Lines | What it does |
|---|---|---|
| `main.rs` | 35 | Finds the repo root, sets up the terminal, runs the app |
| `app.rs` | 1357 | `App`: all state, layout, key routing, popups, hints, tabs, the worker's results |
| `commands.rs` | 414 | The command line's `:git` and `:!`: running in the background or in the terminal, and the output popup |
| `repos.rs` | 203 | Finding the repos `:cd` switches between |
| `git.rs` | 661 | Every git call, returning typed data, plus the parsers |
| `worker.rs` | 195 | The background thread for slow reads |
| `diff.rs` | 1562 | Unified diff and conflict parsing into side-by-side rows, line pairing, word marks, the diff pane (side by side or unified, with its minimap and change counter), the colour cache |
| `highlight.rs` | 139 | Syntax highlighting with syntect |
| `graph.rs` | 1176 | Lane layout (a pure function), colours, the Graph pane |
| `branches.rs` | 436 | The Branches pane |
| `staging.rs` | 286 | The Changes pane |
| `compare.rs` | 166 | The Compare view |
| `file_list.rs` | 117 | Compare's file list, and the file row shared with other panes |
| `details.rs` | 150 | The commit details preview |
| `picker.rs` | 520 | The fuzzy-searchable list behind the Branches pane and the pickers |
| `help.rs` | 300 | The help popup, as data |
| `ui.rs` | 150 | Shared drawing helpers, colours and key hints |

Dependencies: `ratatui` (with crossterm), `anyhow`, and `syntect`, with its pure-Rust regex engine. Dependencies are optimised even in debug builds, since highlighting is too slow otherwise. The theme, `assets/vscode-dark-modern.tmTheme`, holds VS Code's Dark Modern colours, as Zed's "VSCode Dark Modern" theme has them.

### Patterns worth keeping

- **Panels return actions.** Each pane has `handle_key(key) -> Action` and `render(frame, area, focused)`. Panes never touch each other: they return an `Action` (`OpenDiff`, `PreviewDiff`, `Refresh`, `Compare`…), and `App::apply` carries it out. This avoided most fights with the borrow checker.
- **`git.rs` is the only place that knows git is a subprocess.** Everything else gets typed data. Git runs with no stdin, so nothing can wait for hidden input, and with `GIT_OPTIONAL_LOCKS=0`, so reads don't take the index lock.
- **`DiffSource`** names where a diff comes from (unstaged, staged, untracked, conflict, a commit, two refs), so one diff pane serves every feature.
- **Pure functions for the hard parts**, unit tested without a terminal: the graph's lane layout, diff parsing, line pairing, word marks, conflict parsing, the fuzzy matcher.

### Concurrency

- **The worker thread** (`worker.rs`) does the slow reads: the 500 ms auto-refresh poll (the refs fingerprint, `git status`, the open diff) and syntax highlighting. The UI hands it jobs through a small queue and picks up results from a channel every 30 ms, so keys never wait for it.
- **Only reads happen on it.** Anything that changes the repo, and the full refresh after it, stays on the UI thread, in order, so a change shows at once.
- **Stale results are dropped:** each full refresh bumps a generation counter, and a poll asked for before the latest refresh is ignored. Colours only apply to the exact diff text they were made from.
- **Short-lived parallelism** with `std::thread::scope`: the panes' refreshes, the graph's log and unpushed check, a poll's git calls, a comparison's refresh, and the two file versions read for highlighting all run at once.

### Speed

Most of the time goes into starting git processes and into highlighting. Measured on the author's Mac, release builds:

- A full refresh takes about 15 ms, down from 45 ms: the branch list is fetched once, refs are resolved in one `rev-parse`, the checked-out branch is read from `.git/HEAD`, and the rest runs in parallel.
- Moving between files with a diff open takes about 1 ms until the redraw starts, since the diffs one key away (`j`, `k`, `g`, `G`, and `r` in Compare) are prefetched and coloured into a cache of up to 20,000 rows, and shown from it without asking git.
- Memory is about 24 MB, most of it syntect's grammars.

The measuring scripts lived in the git-ignored `target/bench/`. `docs/screenshots.py` shows how to drive gclic in a pseudo-terminal and read its screen.

### Git edge cases

- **Merge conflicts:** a conflicted file previews as its two sides, read from its conflict markers, with the file's own line numbers so `o` opens the editor on the conflict. A diff3 base section is skipped. `d` isn't offered on it, and the Changes title says when a merge, rebase, cherry-pick or revert is in progress. Conflicted files get their own section at the top of the Changes pane, as VS Code's "Merge Changes", shown only while there are any.
- **Worktrees:** a branch checked out in another worktree shows `in ../name` and can't be checked out here. gclic stays in the worktree it started in.
- **Submodules:** shown as one changed entry with git's "Subproject commit" diff. `d` isn't offered, as a plain restore doesn't touch them.

### Platform assumptions

- The clipboard goes through `arboard`, which works on macOS, Windows and Linux.
- The graph's beads are the branch-drawing characters (U+F5EE to U+F60D) in Ghostty and kitty, which draw them themselves, told apart by `TERM_PROGRAM` and `TERM`. WezTerm draws them only in nightly builds (since its PR #6873, April 2025), so it isn't detected. Elsewhere they're plain `●`/`○`, whose lines don't reach them.
- `o` runs `zed path:line`, and says so if `zed` isn't on the `PATH`.
- CI (`.github/workflows/ci.yml`) runs clippy and the tests on macOS and Windows. Pushing a version tag such as `v0.2.0` runs the release workflow (`release.yml`): it checks the tag against `Cargo.toml` and publishes a Windows zip as a GitHub Release.

## Decisions, and why

- **Graph: one row per commit.** Beads are branch-drawing characters with lines to their cell's edges, so consecutive beads join without spacer rows, and forks and joins sit on the bead's row (`●─╮`). A commit takes the colour of the first ref reaching it, trying refs with no commits of their own first, so whichever side is ahead shows in its own colour. Unpushed commits are hollow.
- **The preview is entered, not tabbed to.** `h`/`l` stay free for scrolling inside a diff, and moving through a list with the preview open is the common case.
- **The hint bar is contextual.** It only lists what applies, the key for a destructive action is red in popups, and global keys come last after a separator.
- **Line pairing by similarity.** Removed and added lines pair to maximise their total similarity in order, so an added line doesn't shift every pair below it. Positional pairing breaks the word marks as soon as a change adds a line.
- **Compare is a tab.** It's a different view of the repo, not a pane, and the tab keeps a comparison around without losing it. `c` starts one from wherever you are, and closing it asks first.
- **Git's status letters, VS Code's colours.** `?` untracked and `U` conflicted, as git has them, coloured as VS Code's git panel colours files.
- **Zed only, for now,** on `o`, run in the background while gclic carries on.
- **Compare goes *from* one ref *to* another,** not *before* and *after*: a comparison is a trip through the commit graph, and the two refs need not be ordered in time.
- **Diffs are side by side or unified depending on the room,** side by side when each side gets 80 columns of text. `s` overrides that for the rest of the session. It used to stage in Changes too, as an alias of `space`, and gave that up to work from every pane. Unified is made from the same rows, each change's removed lines then its added ones, so pairing, word marks and colours carry over, and switching keeps the active row.
- **The minimap measures the file, not the diff,** so it looks the same with the whole file or just the changes, and shows where the hunks sit in it. The file's length comes free with highlighting, which reads both versions anyway, and until then the diff's last line stands in.
- **Rare operations are commands, not keys.** `:` opens a command line, so deliberate, occasional operations such as `:cd` don't use up single keys. It's also where running git commands from gclic would go.
- **Two ways to run a command.** `:git …` runs captured, in the background, with no terminal: the editor, credential prompts and SSH passphrases are turned off, so a command that would ask fails at once rather than hang or draw over the TUI, and its error says to use `:!`. It runs in its own process group, so `ctrl-c` stops the helpers git starts too. `:!…` gives the terminal to any command, as vim's `:!` does, which is how editors, `add -p` and passphrases work, without gclic emulating a terminal. gclic waits for enter only if the command printed something, which it tells by the cursor: full-screen programs restore it on exit, and printing moves it. The screen is cleared first, so printing can't scroll it back into place.
- **Switching repos starts the app afresh.** Every git call runs in the process's working directory, so `:cd` moves it to the new repo and rebuilds the app state, keeping only how the user chose to show diffs. The old worker stops once nothing listens for its results, which are about the old repo anyway. The repos are found by reading folders, five deep, not inside a repo and not in hidden or dependency folders, which takes a few tens of milliseconds on a typical `~/Code`, so it runs on demand. The search folder stays the one gclic started with, the repo's parent or where gclic started outside a repo, so `:cd` lists the same repos wherever the user has switched to. `..` moves it up, for the rest of the session. `:cd name` skips the finder when the name can only mean one repo: an exact path or folder name, or the only fuzzy match.
- **A diff on screen takes `n`/`N`, `e` and `s` from any pane,** so its changes can be stepped through while browsing files, and from then on its active line shows, as that's where the user is. The graph's own `n`/`N` between commits only work with no diff open.
- **Commit details as a preview,** following the selected commit, rather than a popup.

## Considered and declined

- **Push, pull and fetch:** one command each with nothing to see. They need the network, can ask for credentials, and lead towards a complete client.
- **Switching worktrees inside gclic:** your shell and editor would stay in the old folder. The model is one terminal tab and editor window per worktree, which is how VS Code does it too, a window per folder.
- **A three-way merge editor:** resolving is for an editor. gclic shows the conflict and opens it in one. VS Code's merge editor works as `git mergetool`.
- **Refreshing on terminal focus:** at 500 ms polling on the worker it adds nothing noticeable.
- **Manual pane resizing, a vim-style cursor in diffs, editors other than Zed, a fallback for terminals without the bead characters, a quick first highlighting pass, showing a diff3 base:** each deferred until it's actually missed.

## Notes for the rewrite

- **`app.rs` has become the place everything lands:** focus, tabs, the preview, popups, hints, the worker's results. It could split into tab state, the preview and popups.
- **Some state is kept consistent by convention:** which preview is showing, which tab, focus and last focus. The tab switch moves the right fields over by hand. Enums could make the wrong combinations impossible.
- **Keep:** the `Action` pattern, `git.rs` with typed results, the worker with tagged results, and the pure, tested layout and parsing functions.
- **Open questions:** should Compare's file list stay the direct difference (`git diff A B`), or be PR-style (`A...B`)? Should the commit box gain a cursor, or open an editor? Should the graph keep 500 commits, or stop where the shown branches meet?
