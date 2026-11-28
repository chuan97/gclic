//! A patch of one change of a diff, for staging or unstaging just that
//! change: what `git add -p` writes when it splits a hunk.

use std::collections::HashSet;

/// The lines a change is made of, by their numbers: the old file's lines it
/// removes, and the new file's lines it adds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChangeLines {
    pub old: HashSet<usize>,
    pub new: HashSet<usize>,
}

/// Which side of the diff the patch is applied to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    /// To the diff's old side, which then gains the change: staging, from
    /// the index to the working tree's diff.
    Forward,
    /// To the diff's new side, which then loses the change, applied with
    /// `-R`: unstaging, from HEAD to the index's diff.
    Reverse,
}

/// A patch of `change`, out of `diff`, a unified diff of the file at `path`
/// as git prints it, to apply in `direction`. None if the change isn't in
/// it.
///
/// The patch has the one hunk the change is in. That hunk's other changes
/// must read as the side the patch is applied to has them: going forward,
/// their removed lines are still there, so they become context, and their
/// added lines aren't yet, so they go. In reverse it's the other way round.
pub fn change_patch(
    diff: &str,
    path: &str,
    change: &ChangeLines,
    direction: Direction,
) -> Option<String> {
    let mut hunk: Option<Hunk> = None;
    // Split on `\n` alone, rather than with `lines()`, which also drops a
    // `\r` before it: in a file with CRLF line endings, the patch's lines
    // must keep theirs to match the file's.
    for line in diff.split_terminator('\n') {
        if line.starts_with("@@ ") {
            if let Some(done) = hunk.take().filter(|hunk| hunk.has_change) {
                return Some(done.patch(path, direction));
            }
            hunk = Some(Hunk::start(line)?);
        } else if let Some(hunk) = hunk.as_mut() {
            hunk.add(line, change, direction);
        }
    }
    hunk.filter(|hunk| hunk.has_change)
        .map(|hunk| hunk.patch(path, direction))
}

/// A hunk of the patch being written.
struct Hunk {
    /// The line each side starts at: the first line in the hunk, or for a side
    /// with no lines, the one after which they'd go.
    old_first: usize,
    new_first: usize,
    /// The line numbers of the next line on each side.
    old_number: usize,
    new_number: usize,
    lines: Vec<String>,
    /// Whether the last line read was kept, for the note that it has no
    /// newline, which goes with it.
    last_kept: bool,
    has_change: bool,
}

impl Hunk {
    fn start(header: &str) -> Option<Hunk> {
        let mut parts = header.split_whitespace().skip(1);
        let mut range = |prefix: char| -> Option<(usize, usize)> {
            let range = parts.next()?.strip_prefix(prefix)?;
            let (start, count) = range.split_once(',').unwrap_or((range, "1"));
            Some((start.parse().ok()?, count.parse().ok()?))
        };
        let (old_start, old_count) = range('-')?;
        let (new_start, new_count) = range('+')?;
        // A side with no lines names the line before where they'd be.
        let first = |start: usize, count: usize| if count == 0 { start + 1 } else { start };
        Some(Hunk {
            old_first: first(old_start, old_count),
            new_first: first(new_start, new_count),
            old_number: first(old_start, old_count),
            new_number: first(new_start, new_count),
            lines: Vec::new(),
            last_kept: false,
            has_change: false,
        })
    }

    fn add(&mut self, line: &str, change: &ChangeLines, direction: Direction) {
        let forward = direction == Direction::Forward;
        let kept = if let Some(text) = line.strip_prefix('-') {
            let chosen = change.old.contains(&self.old_number);
            self.old_number += 1;
            self.has_change |= chosen;
            match (chosen, forward) {
                (true, _) => Some(line.to_string()),
                // Still on the side it's applied to.
                (false, true) => Some(format!(" {text}")),
                (false, false) => None,
            }
        } else if let Some(text) = line.strip_prefix('+') {
            let chosen = change.new.contains(&self.new_number);
            self.new_number += 1;
            self.has_change |= chosen;
            match (chosen, forward) {
                (true, _) => Some(line.to_string()),
                (false, true) => None,
                (false, false) => Some(format!(" {text}")),
            }
        } else if line.starts_with('\\') {
            // "\ No newline at end of file" belongs to the line before it.
            if self.last_kept {
                self.lines.push(line.to_string());
            }
            return;
        } else {
            self.old_number += 1;
            self.new_number += 1;
            Some(line.to_string())
        };
        self.last_kept = kept.is_some();
        self.lines.extend(kept);
    }

    fn patch(&self, path: &str, direction: Direction) -> String {
        let count = |kind: char| {
            self.lines
                .iter()
                .filter(|line| line.starts_with(kind) || line.starts_with(' '))
                .count()
        };
        let (old_count, new_count) = (count('-'), count('+'));
        // Only this hunk is in the patch, so both sides start where the side
        // it's applied to has the hunk.
        let first = match direction {
            Direction::Forward => self.old_first,
            Direction::Reverse => self.new_first,
        };
        let start = |count: usize| if count == 0 { first - 1 } else { first };
        let path = quoted(path);
        let mut patch = format!(
            "--- a/{path}\n+++ b/{path}\n@@ -{},{old_count} +{},{new_count} @@\n",
            start(old_count),
            start(new_count),
        );
        for line in &self.lines {
            patch.push_str(line);
            patch.push('\n');
        }
        patch
    }
}

/// A path as git writes it in a patch: as it is, unless it has a character
/// that needs quoting.
fn quoted(path: &str) -> String {
    if !path.contains(['"', '\\', '\n', '\t']) {
        return path.to_string();
    }
    let escaped = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{self, DiffSource};

    fn lines(old: &[usize], new: &[usize]) -> ChangeLines {
        ChangeLines {
            old: old.iter().copied().collect(),
            new: new.iter().copied().collect(),
        }
    }

    /// A hunk with two changes: line 2 edited, and a line added after line 4.
    const TWO_CHANGES: &str = "\
diff --git a/f b/f
--- a/f
+++ b/f
@@ -1,5 +1,6 @@
 one
-two
+TWO
 three
 four
+four and a half
 five
";

    #[test]
    fn staging_one_change_turns_the_others_into_what_the_index_has() {
        let patch = change_patch(TWO_CHANGES, "f", &lines(&[2], &[2]), Direction::Forward).unwrap();
        assert_eq!(
            patch,
            "--- a/f\n+++ b/f\n@@ -1,5 +1,5 @@\n one\n-two\n+TWO\n three\n four\n five\n"
        );
        let patch = change_patch(TWO_CHANGES, "f", &lines(&[], &[5]), Direction::Forward).unwrap();
        assert_eq!(
            patch,
            "--- a/f\n+++ b/f\n@@ -1,5 +1,6 @@\n one\n two\n three\n four\n+four and a half\n five\n"
        );
    }

    #[test]
    fn unstaging_one_change_keeps_the_others_as_the_index_has_them() {
        let patch = change_patch(TWO_CHANGES, "f", &lines(&[2], &[2]), Direction::Reverse).unwrap();
        assert_eq!(
            patch,
            "--- a/f\n+++ b/f\n@@ -1,6 +1,6 @@\n one\n-two\n+TWO\n three\n four\n four and a half\n five\n"
        );
    }

    #[test]
    fn a_change_not_in_the_diff_makes_no_patch() {
        assert_eq!(
            change_patch(TWO_CHANGES, "f", &lines(&[9], &[9]), Direction::Forward),
            None
        );
        assert_eq!(
            change_patch("", "f", &lines(&[1], &[]), Direction::Forward),
            None
        );
    }

    #[test]
    fn a_later_hunk_starts_where_the_side_applied_to_has_it() {
        // The first hunk adds two lines, so the second is two lines further
        // down in the new file than in the old.
        let diff = "@@ -1,2 +1,4 @@\n a\n+new\n+new\n b\n@@ -10,3 +12,3 @@\n j\n-k\n+K\n l\n";
        let forward = change_patch(diff, "f", &lines(&[11], &[13]), Direction::Forward).unwrap();
        assert!(
            forward.contains("@@ -10,3 +10,3 @@\n j\n-k\n+K\n l\n"),
            "{forward}"
        );
        let reverse = change_patch(diff, "f", &lines(&[11], &[13]), Direction::Reverse).unwrap();
        assert!(reverse.contains("@@ -12,3 +12,3 @@\n"), "{reverse}");
    }

    #[test]
    fn a_side_left_without_lines_names_the_line_before() {
        // Lines added to an empty file: the old side has none.
        let diff = "@@ -0,0 +1,2 @@\n+a\n+b\n";
        let patch = change_patch(diff, "f", &lines(&[], &[1, 2]), Direction::Forward).unwrap();
        assert!(patch.contains("@@ -0,0 +1,2 @@\n+a\n+b\n"), "{patch}");
        let patch = change_patch(diff, "f", &lines(&[], &[1, 2]), Direction::Reverse).unwrap();
        assert!(patch.contains("@@ -0,0 +1,2 @@\n"), "{patch}");
    }

    #[test]
    fn the_missing_newline_note_goes_with_its_line() {
        let diff = "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+B\n\\ No newline at end of file\n";
        // Staging it, both lines and both notes go in.
        let patch = change_patch(diff, "f", &lines(&[2], &[2]), Direction::Forward).unwrap();
        assert!(
            patch.ends_with(
                " a\n-b\n\\ No newline at end of file\n+B\n\\ No newline at end of file\n"
            ),
            "{patch}"
        );
        // An added line left out takes its note with it.
        let diff = "@@ -1 +1,2 @@\n-a\n\\ No newline at end of file\n+a\n+b\n\\ No newline at end of file\n";
        let patch = change_patch(diff, "f", &lines(&[1], &[1, 2]), Direction::Forward).unwrap();
        assert!(
            patch.ends_with(
                "-a\n\\ No newline at end of file\n+a\n+b\n\\ No newline at end of file\n"
            )
        );
    }

    #[test]
    fn a_file_with_crlf_line_endings_keeps_them_in_the_patch() {
        let repo = crate::test_repo::TestRepo::new();
        let crlf =
            |words: &[&str]| -> String { words.iter().map(|word| format!("{word}\r\n")).collect() };
        repo.write("f.txt", &crlf(&["one", "two", "three", "four"]));
        repo.commit("CRLF");
        repo.write("f.txt", &crlf(&["one", "TWO", "three", "FOUR"]));
        let diff = git::diff(&repo, &DiffSource::Unstaged("f.txt".to_string()), false).unwrap();
        let patch = change_patch(&diff, "f.txt", &lines(&[2], &[2]), Direction::Forward).unwrap();
        git::apply_to_index(&repo, &patch, false).unwrap();
        let staged = git::diff(&repo, &DiffSource::Staged("f.txt".to_string()), false).unwrap();
        assert!(
            staged.contains("+TWO\r\n") && !staged.contains("FOUR"),
            "{staged}"
        );
    }

    #[test]
    fn paths_that_need_it_are_quoted() {
        assert_eq!(quoted("a file.rs"), "a file.rs");
        assert_eq!(quoted("say \"hi\".txt"), "\"say \\\"hi\\\".txt\"");
    }
}
