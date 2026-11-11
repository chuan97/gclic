# gclic (git CLI companion)

**gclic**, pronounced *g-click*, is a TUI intended to complement git's CLI for operations that are best done visually.

If you are looking for a mature and feature complete git TUI check out [lazygit](https://github.com/jesseduffield/lazygit).
gclic is heavily inspired by the features I like and have grown accustomed to in [VS Code's git client](https://code.visualstudio.com/docs/sourcecontrol/overview) and the [GitLens](https://marketplace.visualstudio.com/items?itemName=eamodio.GitLens) extension and that I miss in [Zed](https://zed.dev). gclic intentionally leaves out operations that in my opinion gain little from being visual, such as pushing, pulling and fetching, merging and rebasing, or creating branches, which it leaves to git's CLI. Others, like resolving conflicts, certainly benefit from being visual, but belong in a full-fledged editor, which gclic does not aspire to be: it shows the conflict, and opens it in your editor.

## Features

- **[Staging and commit](#staging-and-commit):** browse changed files, stage, unstage or restore them, and commit.
- **[Branch graph](#branch-graph):** navigate a commit graph of any set of branches, open a commit's files and diffs, read its details and copy its sha.
- **[Branches](#branches):** fuzzy search and check out branches, and see how far each one is ahead of or behind its remotes.
- **[Diffs](#diffs):** read any file's changes side by side or unified, with syntax highlighting, the changed words marked, and a minimap of the whole file.
- **[Compare](#compare):** pick any two refs to see how far they have diverged and which files differ.
- **[Merge conflicts](#merge-conflicts):** see each conflict as your side against theirs, and open it in your editor to resolve it.
- **[Keyboard driven](#keyboard-driven):** with Vim-style keybindings, and the keys that apply always shown at the bottom.

## Installation

**On Windows**, download `gclic-v<version>-windows-x86_64.zip` from the [releases](https://github.com/chuan97/gclic/releases) and extract `gclic.exe`. To run it from any folder, put it in a folder on your `PATH`, for example a `bin` folder in your user folder:

1. Create the folder `C:\Users\<you>\bin` and move `gclic.exe` into it.
2. Open the Start menu, search for *Edit environment variables for your account*, and open it.
3. Under *User variables*, select `Path`, choose *Edit*, then *New*, and enter `%USERPROFILE%\bin`.
4. Choose *OK* in both windows, and open a new terminal.

gclic isn't signed yet, so the first time Windows may say it "protected your PC": choose *More info*, then *Run anyway*. It needs [Git for Windows](https://git-scm.com/download/win), and looks best in [Windows Terminal](https://aka.ms/terminal).

**From source**, on any platform with [Rust](https://rustup.rs) installed:

```sh
cargo install --git https://github.com/chuan97/gclic
```

Then run `gclic` inside a git repository. Press `?` for help.

**Note:** As of now, gclic is fully *vibe-coded*. It is barely a working prototype, and has only been tested in macOS with [Ghostty](https://ghostty.org) as the terminal emulator. It is best run in a horizontally split pane, with gclic in one pane and git's CLI in the other.

gclic currently expects:
- **git** on the `PATH`.
- **[Zed](https://zed.dev)**, to open files in an editor with `o`.

In [Ghostty](https://ghostty.org) and [kitty](https://sw.kovidgoyal.net/kitty/), which draw git graph symbols themselves, the graph's beads join up with their lines, as in the screenshots. Other terminals, including Windows Terminal, get plain beads, and look the same otherwise.

## Staging and commit

The Changes pane lists staged and unstaged files, each name followed by its folder. `space` stages or unstages one, `a`/`A` all of them, `d` restores a file (after asking), and `c` commits what's staged, with `ctrl-enter` to confirm. `enter` shows a file's diff on the right, and the diff follows as you move through the list.

![The Changes pane with a file's diff on the right](docs/images/staging.svg)

## Branch graph

The graph shows the checked-out branch and its upstream by default, or any branches you pick with `b`. Commits not yet pushed are hollow. `enter` opens a commit's files, `i` shows its details on the right, `y` copies its sha, and `n`/`N` jump between commits.

![The graph with a commit's files open and its details on the right](docs/images/graph.svg)

## Branches

The Branches pane shows the checked-out branch, and when focused, every branch. Each one shows how far it is ahead (↑) or behind (↓) its upstream. `enter` checks a branch out, `i` searches, `space` lists a branch's remotes, and `c` compares it with the checked-out branch.

![The Branches pane with a branch's remotes open](docs/images/branches.svg)

## Diffs

Diffs are syntax highlighted with VS Code's Dark Modern colours, with the words that changed marked. They're side by side when the pane is wide enough for about 80 columns of text a side, and unified otherwise. A minimap down the right edge shows the whole file, its removed lines on the left and its added ones on the right, with the part on screen lighter. The bottom border counts the changes, and says which one you're on.

From any pane, while a diff is showing, `n`/`N` jump between its changes, `e` switches between the whole file and just the changes, and `s` between side by side and unified. A second `enter` moves into a diff: `j`/`k` move the active line, `h`/`l` scroll sideways, and `o` opens the file in Zed at the active line.

![A commit's diff, with the active line a few rows down](docs/images/diff.svg)

## Compare

The Compare tab, reached with `tab`, shows how two refs differ, going *from* one *to* the other, a trip through the commit graph rather than an order in time: how far the to is ahead of and behind the from, their common ancestor, and the files that differ, in a tree of their folders, each with its diff. `c` starts a comparison, from the Branches pane, the graph, or the tab itself, and `r` swaps from and to.

![Comparing main with hourly](docs/images/compare.svg)

## Merge conflicts

While a merge, rebase or cherry-pick is stopped on conflicts, the Changes pane says so, and lists the conflicted files in a section of their own, above the staged ones. A conflicted file's preview shows each conflict as your side (green) against theirs (blue). Resolve them in your editor with `o`, which opens the file at the conflict, then mark the file resolved with `space`, which stages it.

![A merge conflict, yours against theirs](docs/images/conflict.svg)

## Keyboard driven

Every action has a key, mostly Vim-style. `h`/`l` move between panes, marked `[h]` and `[l]` on their borders. The bar at the bottom lists the keys that apply right now, and `?` shows them all.

![The help, opened from the graph](docs/images/help.svg)

---

The screenshots are made from gclic itself, by `docs/screenshots.py`, on a demo repo it builds.

## License

gclic is under the [MIT License](LICENSE). Its colours are those of VS Code's Dark Modern theme, from [VS Code](https://github.com/microsoft/vscode), Copyright (c) Microsoft Corporation, also under the MIT License.
