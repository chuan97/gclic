# gclic (git CLI companion)

**gclic**, pronounced *g-click*, is a TUI intended to complement git's CLI for operations that are best done visually.

## Features

- **Staging and commit:** browse changed files, stage, unstage or restore them, and commit.
- **Branch graph:** navigate a commit graph of any set of branches, open a commit's files and diffs, read its details and copy its sha.
- **Branches:** fuzzy search and check out branches, and see how far each one is ahead of or behind its remotes.
- **Compare:** pick any two refs to see how far they have diverged and which files differ.
- **Diffs:** read any file's changes in a side-by-side view.
- **Keyboard driven:** with Vim-style keybindings.

## Installation

```sh
cargo install --git https://github.com/chuan97/gclic --branch mvp
```

Then run `gclic` inside a git repository. Press `?` for help.
