//! Diffs already coloured, kept so that going back to one, or to one
//! prefetched, shows its colours straight away.

use super::parse::*;
use crate::git::DiffSource;

/// Rows with their syntax colours, and the length of the file the minimap
/// measures, which colouring them finds out. Zero while not known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Colored {
    pub rows: Vec<Row>,
    pub file_lines: usize,
}

/// The most rows the colour cache keeps, over all its diffs. Enough for the
/// diffs around the cursor and a good few recent ones, in a few MB.
const CACHE_ROWS: usize = 20_000;

/// A file's diff as shown: of the whole file, or just the changes.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffView {
    pub source: DiffSource,
    pub whole: bool,
}

/// Diffs already coloured, so that going back to one, or to one prefetched,
/// shows its colours straight away. The most recently used come last.
#[derive(Default)]
pub struct ColorCache {
    entries: Vec<(DiffView, String, Colored)>,
}

impl ColorCache {
    /// The coloured rows of this diff, if they were made from this same text.
    pub fn get(&mut self, view: &DiffView, text: &str) -> Option<Colored> {
        let position = self
            .entries
            .iter()
            .position(|(v, t, _)| v == view && t == text)?;
        let entry = self.entries.remove(position);
        let colored = entry.2.clone();
        self.entries.push(entry);
        Some(colored)
    }

    /// The most recent version of this diff, whatever its text.
    pub fn latest(&mut self, view: &DiffView) -> Option<(String, Colored)> {
        let position = self.entries.iter().position(|(v, ..)| v == view)?;
        let entry = self.entries.remove(position);
        let found = (entry.1.clone(), entry.2.clone());
        self.entries.push(entry);
        Some(found)
    }

    /// Whether some version of this diff is cached. A working-tree file may
    /// have changed since, which showing it will find out.
    pub fn contains(&self, view: &DiffView) -> bool {
        self.entries.iter().any(|(v, ..)| v == view)
    }

    /// Adds a diff, replacing an older version of it and dropping the least
    /// recently used ones past the limit.
    pub fn insert(&mut self, view: DiffView, text: String, colored: Colored) {
        self.entries.retain(|(v, ..)| *v != view);
        self.entries.push((view, text, colored));
        let total = |entries: &[(DiffView, String, Colored)]| -> usize {
            entries.iter().map(|(.., colored)| colored.rows.len()).sum()
        };
        while self.entries.len() > 1 && total(&self.entries) > CACHE_ROWS {
            self.entries.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::testing::colored;

    fn view(name: &str, whole: bool) -> DiffView {
        DiffView {
            source: DiffSource::Unstaged(name.to_string()),
            whole,
        }
    }

    #[test]
    fn the_cache_only_matches_the_same_text() {
        let mut cache = ColorCache::default();
        let view = view("f", false);
        cache.insert(
            view.clone(),
            "v1".to_string(),
            colored(parse_diff("@@ -1 +1 @@\n-a\n+b")),
        );
        assert!(cache.get(&view, "v1").is_some());
        assert!(cache.get(&view, "v2").is_none());
        assert!(cache.contains(&view));
    }

    #[test]
    fn the_cache_keeps_a_files_whole_and_changes_only_diffs_apart() {
        let mut cache = ColorCache::default();
        cache.insert(view("f", false), "changes".to_string(), Colored::default());
        cache.insert(view("f", true), "whole".to_string(), Colored::default());
        assert_eq!(
            cache
                .latest(&view("f", false))
                .map(|(text, _)| text)
                .as_deref(),
            Some("changes")
        );
        assert_eq!(
            cache
                .latest(&view("f", true))
                .map(|(text, _)| text)
                .as_deref(),
            Some("whole")
        );
    }

    #[test]
    fn the_cache_drops_the_least_recently_used_past_its_limit() {
        let mut cache = ColorCache::default();
        let file = |name: &str| view(name, false);
        let rows = vec![Row::Heading("x".to_string()); CACHE_ROWS / 2];
        cache.insert(file("a"), String::new(), colored(rows.clone()));
        cache.insert(file("b"), String::new(), colored(rows.clone()));
        cache.get(&file("a"), ""); // a is now the most recently used
        cache.insert(file("c"), String::new(), colored(rows));
        assert!(cache.contains(&file("a")) && cache.contains(&file("c")));
        assert!(!cache.contains(&file("b")));
    }
}
