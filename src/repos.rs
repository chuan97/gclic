//! Finding the git repos under a folder, for `:cd` to switch between them.
//! It only reads folders, so it needs no git calls.

use std::fs;
use std::path::{Path, PathBuf};

/// How many folders down from the search folder a repo can be.
const MAX_DEPTH: usize = 5;
/// Folders that hold dependencies or build output, never a repo worth opening,
/// and can be huge.
const SKIPPED: &[&str] = &["node_modules", "target", "venv", "__pycache__", "build", "dist"];

/// The entry that moves the search up a folder.
pub const UP: &str = "..";

/// The repos in `root` and the folders under it, sorted by path. A repo is a
/// folder with a `.git` folder, or a `.git` file as worktrees have. Repos
/// inside a repo, such as submodules, are left out, as are hidden folders.
pub fn find(root: &Path) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    search(root, 0, &mut repos);
    repos.sort();
    repos
}

fn search(folder: &Path, depth: usize, repos: &mut Vec<PathBuf>) {
    if folder.join(".git").exists() {
        repos.push(folder.to_path_buf());
        return;
    }
    if depth == MAX_DEPTH {
        return;
    }
    // A folder that can't be read is skipped, as it can't hold a repo for us.
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Not following links to folders, which could lead in circles.
        let is_folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if is_folder && !name.starts_with('.') && !SKIPPED.contains(&name.as_ref()) {
            search(&entry.path(), depth + 1, repos);
        }
    }
}

/// How a repo is listed: its path from the search folder, or its name if it's
/// the search folder itself.
pub fn label(root: &Path, repo: &Path) -> String {
    match repo.strip_prefix(root) {
        Ok(relative) if relative.as_os_str().is_empty() => {
            repo.file_name().map_or_else(|| repo.display().to_string(), |name| name.to_string_lossy().into_owned())
        }
        Ok(relative) => relative.display().to_string(),
        Err(_) => repo.display().to_string(),
    }
}

/// What the finder lists for a search folder: `..` for the folder above, if
/// there is one, then each repo, as its label and where it is.
pub fn entries(root: &Path) -> Vec<(String, PathBuf)> {
    let up = root.parent().map(|parent| (UP.to_string(), parent.to_path_buf()));
    let repos = find(root).into_iter().map(|repo| (label(root, &repo), repo));
    up.into_iter().chain(repos).collect()
}

/// Where `:cd` with a name leads.
#[derive(Debug, PartialEq)]
pub enum Target {
    /// To this repo, the only one the name can mean.
    Repo(PathBuf),
    /// To the finder, as several repos match.
    Choose,
    None,
}

/// Which repo a name typed after `:cd` means: the one whose path or folder
/// name it is exactly, or failing that the only one it fuzzy matches.
pub fn target(name: &str, entries: &[(String, PathBuf)]) -> Target {
    let repos: Vec<&(String, PathBuf)> = entries.iter().filter(|(label, _)| label != UP).collect();
    let folder_name = |repo: &Path| repo.file_name().map(|name| name.to_string_lossy().into_owned());
    let exact = repos.iter().find(|(label, repo)| label == name || folder_name(repo).as_deref() == Some(name));
    if let Some((_, repo)) = exact {
        return Target::Repo(repo.clone());
    }
    let mut matching = repos.iter().filter(|(label, _)| crate::picker::fuzzy_score(name, label).is_some());
    match (matching.next(), matching.next()) {
        (Some((_, repo)), None) => Target::Repo(repo.clone()),
        (Some(_), Some(_)) => Target::Choose,
        (None, _) => Target::None,
    }
}

/// A path as it's shown, with `~` for the home folder.
pub fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

/// Where `:cd` looks for repos: the folder holding the repo gclic is in, so
/// its siblings are listed, or the folder itself if it isn't in a repo.
pub fn search_root(start: &Path, repo_root: Option<&Path>) -> PathBuf {
    match repo_root {
        Some(root) => root.parent().unwrap_or(root).to_path_buf(),
        None => start.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty folder for one test.
    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gclic-repos-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn repo(folder: &Path, head: &str) {
        fs::create_dir_all(folder.join(".git")).unwrap();
        fs::write(folder.join(".git/HEAD"), head).unwrap();
    }

    #[test]
    fn finds_repos_but_not_inside_them_or_in_skipped_folders() {
        let root = scratch("find");
        repo(&root.join("a"), "ref: refs/heads/main\n");
        repo(&root.join("a/sub"), "ref: refs/heads/main\n"); // a submodule, say
        repo(&root.join("group/b"), "ref: refs/heads/main\n");
        repo(&root.join("node_modules/c"), "ref: refs/heads/main\n");
        repo(&root.join(".hidden/d"), "ref: refs/heads/main\n");
        fs::create_dir_all(root.join("empty")).unwrap();
        assert_eq!(find(&root), [root.join("a"), root.join("group/b")]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worktree_counts() {
        let root = scratch("worktree");
        repo(&root.join("main"), "ref: refs/heads/main\n");
        let worktree_git = root.join("main/.git/worktrees/wt");
        fs::create_dir_all(&worktree_git).unwrap();
        fs::write(worktree_git.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
        fs::create_dir_all(root.join("wt")).unwrap();
        fs::write(root.join("wt/.git"), format!("gitdir: {}\n", worktree_git.display())).unwrap();
        assert_eq!(find(&root), [root.join("main"), root.join("wt")]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repos_are_labelled_by_their_path_from_the_search_folder() {
        let root = scratch("label");
        repo(&root.join("group/b"), "ref: refs/heads/dev\n");
        // Joined a part at a time, as a search finds it, so Windows has its own
        // separator in it.
        let found = root.join("group").join("b");
        assert_eq!(label(&root, &found), Path::new("group").join("b").display().to_string());
        assert_eq!(label(&root, &root), root.file_name().unwrap().to_string_lossy());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_list_starts_with_the_way_up() {
        let root = scratch("entries");
        repo(&root.join("a"), "ref: refs/heads/main\n");
        let entries = entries(&root);
        assert_eq!(entries[0], (UP.to_string(), root.parent().unwrap().to_path_buf()));
        assert_eq!(entries[1], ("a".to_string(), root.join("a")));
        assert!(super::entries(Path::new("/")).iter().all(|(label, _)| label != UP));
        let _ = fs::remove_dir_all(&root);
    }

    fn listed(paths: &[&str]) -> Vec<(String, PathBuf)> {
        let mut entries = vec![(UP.to_string(), PathBuf::from("/"))];
        entries.extend(paths.iter().map(|path| (path.to_string(), Path::new("/code").join(path))));
        entries
    }

    #[test]
    fn a_name_leads_straight_to_the_repo_it_can_only_mean() {
        let entries = listed(&["skycast", "skycast-conflict", "work/gclic"]);
        // Exact, though skycast-conflict matches too.
        assert_eq!(target("skycast", &entries), Target::Repo("/code/skycast".into()));
        // The folder's name, without the folders around it.
        assert_eq!(target("gclic", &entries), Target::Repo("/code/work/gclic".into()));
        // The only fuzzy match.
        assert_eq!(target("conf", &entries), Target::Repo("/code/skycast-conflict".into()));
        assert_eq!(target("sky", &entries), Target::Choose);
        assert_eq!(target("zzz", &entries), Target::None);
    }

    #[test]
    fn the_search_starts_beside_the_repo_or_where_gclic_started() {
        let (code, repo) = (Path::new("/code"), Path::new("/code/gclic"));
        assert_eq!(search_root(Path::new("/code/gclic/src"), Some(repo)), code);
        assert_eq!(search_root(code, None), code);
    }
}
