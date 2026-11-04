# mygc: design notes

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

## MVP plan (as approved)

### Context
A keyboard-driven TUI for the git tasks where a CLI is slow: visual staging and commit, a bead-style branch graph, a fuzzy branch picker, and a two-ref compare. It is also a way to learn Rust, so the code must stay simple enough for the author to follow and grow with.

This first pass builds all four features to find out what we really want. After that, each feature gets refined or rewritten one at a time, so each one lives in its own module.

The repo is a fresh `cargo new` (`src/main.rs` is hello world, no commits). Rust 1.94, git 2.50, macOS.

### Decisions
- **Git backend:** shell out to the `git` CLI. Only `git.rs` knows it is a subprocess; everything else sees typed structs.
- **Graph:** vertical, one bead per row, dim one-line subject, branch labels only at tips.
- **Diff:** side-by-side, in a right-hand pane, with the lists staying on the left. The diff follows the list selection.
- **Comments:** normal comments only, where the logic isn't obvious.
- **Dependencies:** `ratatui` (re-exports crossterm) and `anyhow`. Nothing else: no async, threads, custom traits or unneeded generics. The fuzzy matcher is hand-written (about 25 lines).

### Layout
```
┌ Changes ───────┬ src/main.rs ───────────────────┐
│ Staged (1)     │ 12  let a = 1;  │ 12  let a = 1;│
│  M main.rs     │ 13- let b = 2;  │ 13+ let b = 3;│
│ Changes (2)    │ 14  let c = 4;  │ 14  let c = 4;│
│  M foo.rs      │                 │               │
│  ? new.rs      │                 │               │
├ Graph ─────────┤                 │               │
│ ● feature  fix │                 │               │
│ │ ● origin/fe… │                 │               │
│ ├─╯            │                 │               │
│ ● wire config  │                 │               │
└────────────────┴─────────────────┴───────────────┘
 status line: key hints, or the last error/message
```
The left column takes the full width until a diff is open. `z` zooms the diff to full width.

### Modules (`src/`, about 1500-2000 lines total)
| File | Responsibility |
|---|---|
| `main.rs` | Terminal init/restore (`ratatui::init` restores on panic), event loop |
| `app.rs` | `App` state, focus, top-level layout and key dispatch, applies `Action`s |
| `git.rs` | Every git call, each returning typed data. Includes the parsers |
| `diff.rs` | Unified diff to side-by-side rows; `DiffPane` state and render |
| `staging.rs` | Feature 1: the Changes panel |
| `graph.rs` | Feature 2: lane layout (pure function), Graph panel, commit drill-down |
| `picker.rs` | Fuzzy list popup, single or multi-select. Used by features 2, 3 and 4 |
| `compare.rs` | Feature 4: the compare panel |
| `file_list.rs` | Selectable changed-files list, shared by commit drill-down and compare |

#### The one pattern to learn
Each panel is a plain struct with two methods:
- `handle_key(&mut self, key) -> Action`
- `render(&self, frame, area, focused)`

Panels never touch each other. They return an `Action` enum (`OpenDiff(DiffSource)`, `OpenPicker(..)`, `Refresh`, `Message(..)`...) and `App` applies it. This also avoids most early borrow-checker fights.

`DiffSource` is the enum that makes one diff pane serve every feature:
```rust
enum DiffSource {
    Unstaged(String), Staged(String), Untracked(String),
    Commit { sha: String, path: String },
    Refs { a: String, b: String, path: String },
}
```

The loop is single-threaded: poll for a key (250 ms timeout), handle it, redraw. Git calls block, which is fine at this scale.

### Feature details

#### 1. Staging and commit
- `git status --porcelain=v1 -z --no-renames --untracked-files=all`. Two sections, like VS Code; a partly staged file appears in both.
- Refreshes after every action, on `r`, and every ~2 s (status only, with `GIT_OPTIONAL_LOCKS=0`) so outside edits show up.
- Selection is kept by path across refreshes.
- Stage: `git add -- p`. Unstage: `git reset -q -- p` (unlike `restore --staged`, this also works before the first commit).
- Restore always asks y/n first. Tracked: `git restore -- p`. Untracked: `git clean -f -- p`, with a prompt that says the file will be **deleted**. Not offered on staged entries.
- Commit: `c` opens a multi-line message box (append-only editing), `Ctrl-s` commits, `Esc` cancels and keeps the draft. Runs `git commit -m`, so hooks and signing behave normally. Hook failures appear in an error popup.

#### 2. Graph
- **Default refs:** HEAD and its upstream, falling back to `origin/<branch>`, then HEAD alone.
- **Data:** `git log --topo-order -n 500 --format=%H%x00%P%x00%s <refs>`. A `┊` marks truncation.
- **Lanes:** `layout(commits) -> Vec<Row>` is a pure function, unit tested with no UI. It emits commit rows, plus connector rows (`├─╯`) where lanes fork or merge. Only commit rows are selectable.
- **Color, one rule for any number of refs:** in one pass through the topo-ordered commits, work out which selected refs reach each commit.
  - All refs: dim (shared).
  - Exactly one ref: that ref's color.
  - Some but not all: neutral.
  - So the default view gets local = green, origin = blue, shared = dim, and the color changes at the transition. A single-ref view uses that ref's color.
- **Keys:** `j/k` move, `Enter` opens the commit's files in place of the graph (`Esc` returns), `Enter` on a file opens its diff, `i` toggles a box with full message, sha, author and date, `y` copies the sha (`pbcopy`, macOS only for now), `B` opens the multi-select picker (`Space` toggles; `Enter` with nothing ticked returns to the default view).
- Merge commits diff against their first parent.

#### 3. Branch picker
- `b` opens the picker over local and remote branches (`git for-each-ref`, skipping `*/HEAD`). Type to filter, arrows or `Ctrl-j/k` to move, `Enter` to check out.
- Remote branches use `git switch <name-without-remote>`, which creates a tracking branch, or switches to the local one if it already exists.
- Failures such as a dirty tree conflict appear in the error popup.

#### 4. Compare
- `C` opens the picker twice (A, then B) over branches and tags. If nothing matches, `Enter` uses the typed text as a ref, so shas work. Validated with `git rev-parse --verify`.
- The left column shows `A is N ahead, M behind B`, the merge-base, and the file list. `Enter` opens a split diff, `Esc` exits.
- `git rev-list --left-right --count A...B`, `git merge-base`, `git diff --name-status --no-renames -z A B`.

#### Diff pane
- Parses hunks into rows. A run of `-` lines pairs row by row with the following `+` lines, with blank filler on the shorter side.
- Keys: `Tab` focuses it, `j/k` scroll, `Ctrl-d/u` half page, `n/p` next/prev hunk, `Esc` back to the list (`Esc` in the list closes the diff).
- Commands always pass `--no-ext-diff --no-color`. Binary files show a message. Long lines are clipped.

#### Global keys
`Tab` focus, `b` branches, `B` graph branches, `C` compare, `r` refresh, `?` help, `q` quit.

### Robustness
- On startup: `git rev-parse --show-toplevel`, then change directory there, since status paths are root-relative. Outside a repo, print a clear message and exit before entering the TUI.
- Must handle an unborn HEAD (this repo right now), a detached HEAD, and an empty repo (graph shows "no commits yet").
- Git commands run with stdin null so nothing waits on hidden input.

### Out of scope for the MVP (candidates for later)
Hunk or line staging, fetch/push/pull, merge conflict tools, rename detection (renames show as delete + add), syntax highlighting, word-level diff, horizontal scroll, config or custom keybindings, Linux clipboard, a cursor in the commit box.

### Assumption to confirm when trying it
Compare's file list is the **direct** difference between A and B (`git diff A B`), not the PR-style "changes on B since the merge-base" (`A...B`). Switching is a one-line change.

### Build order
1. `git.rs` with parser tests.
2. `main.rs` + `app.rs` skeleton.
3. `staging.rs` + `diff.rs`.
4. `graph.rs`.
5. `picker.rs` + checkout.
6. `compare.rs`.

### Git handling
Work happens in a worktree if one can be created. This repo has no commits, so that may fail, and I'd then build in place on the shared checkout. In that case I'll ask before committing. No pushing (no remote).

### Verification
- `cargo build`, `cargo clippy`, `cargo test`. Unit tests cover the status, name-status and diff parsers, side-by-side pairing, lane layout (linear, diverged, merge) and the fuzzy matcher.
- A script builds a throwaway repo in the job's temp directory with diverged `feature` / `origin/feature`, a merge, staged, unstaged and untracked files, and a tag.
- Drive the real binary in that repo through `tmux send-keys` and `capture-pane`: stage, unstage, restore, commit, open a diff, walk the graph, open the details box, copy a sha (check `pbpaste`), select branches, check out, compare. Destructive actions run only in the scratch repo.
- Also run it in an empty repo and outside a repo.

## What changed while building the MVP

The plan above is kept as approved. These are the differences in the code:

- **One extra module:** `ui.rs` holds small drawing helpers shared by the panels (panel border, scroll handling, full-width rows, popup centering).
- **Graph selection follows new commits:** if the top commit is selected and nothing is open, a new commit moves the selection to the new tip. Otherwise the selection stays on the same commit. The plan only said to keep it by commit.
- **Remote color:** bright blue instead of blue, which is too dark to read on dark terminals.
- **Auto-refresh pauses while a popup is open,** so it can't replace a dialog you're using.

## First round of feedback on the MVP

Implemented after trying the MVP. Where this differs from the plan above, this wins.

- **Picker modes:** `j`/`k` navigate by default. `i` starts typing, like vim's insert mode. The first `Esc` stops typing, and the next closes the picker.
- **Lines between beads:** every pair of beads is joined by a row of lines, even though this fits about half as many commits on screen.
- **Graph row order:** bead, title, author, branch names.
- **Word-level highlighting in diffs:** paired lines mark the changed words with a slightly stronger red or green, using a longest common subsequence over words. Lines with little in common (under 30%, ignoring whitespace) get no word marks.
- **Commit files as a drop-down:** `Enter` opens a commit's files inline under it, and `j`/`k` walk through them as rows. Several commits can be open at once. `Esc` collapses the selected commit, or all of them from a closed commit. The separate file view is gone, so `file_list.rs` now only serves compare, plus the shared file row.
- **Branch colors, no dimming:** each ref has a color, and its label uses it. With several branches selected, a branch and its remotes share a color. With just one branch and its remote, as in the default view, each gets its own: local green, remote blue. A commit takes the color of the first ref reaching it, trying refs with no commits of their own first. So whichever side is ahead shows in its color against the rest: local ahead makes shared history blue, remote ahead makes it green. This replaces the plan's dim shared commits.
- **Hollow beads:** a commit that isn't on any remote-tracking branch is drawn `○`, and pushed commits `●`.
- **Commit details popup:** `i` opens the commit's message and metadata in a centered popup that scrolls with `j`/`k`, rather than a box at the bottom of the graph. It lives in `details.rs`. It shows the commit it was opened on, because `j`/`k` scroll instead of moving through commits. Counting wrapped lines uses `ratatui`'s `unstable-rendered-line-info` feature, so check that API when upgrading `ratatui`.
- **Remotes in the Branches pane:** a remote branch is hidden if a local branch has its name or tracks it, since selecting it would only switch to that local branch. Remote-only branches stay, dimmed, and checking one out creates a local tracking branch (`git switch --track`). Each local branch shows how many remotes have it. `Space` opens a drop-down listing them, with ahead/behind counts and which one is tracked. A local branch tracks at most one upstream, so the count means remotes that have the branch, not remotes tracking it. The rows are read-only, so `j`/`k` skip them.
- **Branches pane:** checkout moved from a popup to a permanent pane at the top of the left column (`branches.rs`). The checked-out branch shows as `* name` in bold green, and the cursor is a selection bar. `i` searches, as in the picker it shares its list with. Choosing which branches the graph shows stays a popup, opened with `b` in the Graph pane.
- **`h`/`l` switch panes, not `Tab`:** a fixed line of Branches, Changes, Graph, then Diff when open. `Esc` from the diff still returns to the pane it came from.
- **Tick all in the graph's branch picker:** `a` ticks and `A` unticks everything shown, so a search narrows what they affect.
- **Comparing commits by sha:** `p` in the compare pickers pastes the first word on the clipboard, so `y` in the graph, then `C`, `p`, `Enter` compares a commit. Compare shortens shas with `git rev-parse --short`, to keep titles readable.
- **Wrap-around:** `j`/`k` and `h`/`l` wrap past either end, in every list, popup and pane. `g`/`G` and the half-page keys are jumps, so they stop at the ends.
- **Constant refresh:** every 500 ms the app checks `git status` and a fingerprint of refs and HEAD, reloading the graph and compare only when the fingerprint changes. This runs on the UI thread, so a repo with a slow `git status` could make keys lag; the fix would be a background thread.

Still left for the full implementation: syntax highlighting in diffs, which probably means a dependency like `syntect`.

## Questions to answer by trying it

- Should compare's file list stay the direct difference (`git diff A B`), or be PR-style (`A...B`)?
- Is a 40/60 split right for the lists and the diff, and is 40/60 right between Changes and Graph?
- Is the commit box enough with append-only editing, or should it gain a cursor or open `$EDITOR`?
- Should the graph keep the full 500 commits, or stop soon after the point where the selected branches meet?
