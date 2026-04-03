//! Git-based change detection for fast incremental indexing.
//!
//! Uses the `git` CLI to detect which files changed since the last
//! indexed commit, avoiding the need to read and hash every file.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// Result of git-based change detection.
pub struct GitDiff {
    /// Current HEAD commit OID
    pub head_oid: String,
    /// Files that were added or modified since the last indexed commit
    pub changed: Vec<String>,
    /// Files that were deleted since the last indexed commit
    pub deleted: Vec<String>,
}

/// Gets the current HEAD commit OID for the given directory.
/// Returns `None` if not a git repository or git is unavailable.
pub fn head_oid(root: &Path) -> Option<String> {
    git_head_oid(root)
}

/// Detects changed files between the last indexed commit and current state.
///
/// Returns `None` if:
/// - The directory is not a git repository
/// - The git command fails
/// - The last indexed commit is not an ancestor of HEAD
pub fn detect_changes(root: &Path, last_indexed_commit: &str) -> Option<GitDiff> {
    let head_oid = git_head_oid(root)?;

    if !is_ancestor(root, last_indexed_commit, &head_oid) {
        return None;
    }

    let committed_changes = git_diff_tree(root, last_indexed_commit, &head_oid)?;
    let working_changes = git_diff_working(root)?;

    let mut changed_set = HashSet::new();
    let mut deleted_set = HashSet::new();

    for (status, path) in committed_changes.into_iter().chain(working_changes) {
        if status == 'D' {
            changed_set.remove(&path);
            deleted_set.insert(path);
        } else {
            deleted_set.remove(&path);
            changed_set.insert(path);
        }
    }

    Some(GitDiff {
        head_oid,
        changed: changed_set.into_iter().collect(),
        deleted: deleted_set.into_iter().collect(),
    })
}

fn git_head_oid(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|e| {
            tracing::debug!("git rev-parse HEAD failed to spawn: {e}");
            e
        })
        .ok()?;
    if !output.status.success() {
        tracing::debug!("git rev-parse HEAD exited with {}", output.status);
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(root)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn git_diff_tree(root: &Path, from: &str, to: &str) -> Option<Vec<(char, String)>> {
    let output = Command::new("git")
        .args(["diff", "--name-status", "--no-renames", from, to])
        .current_dir(root)
        .output()
        .map_err(|e| {
            tracing::debug!("git diff --name-status failed to spawn: {e}");
            e
        })
        .ok()?;
    if !output.status.success() {
        tracing::debug!("git diff --name-status exited with {}", output.status);
        return None;
    }
    Some(parse_name_status(&String::from_utf8_lossy(&output.stdout)))
}

fn git_diff_working(root: &Path) -> Option<Vec<(char, String)>> {
    let staged = Command::new("git")
        .args(["diff", "--name-status", "--no-renames", "--cached"])
        .current_dir(root)
        .output()
        .ok()?;
    let unstaged = Command::new("git")
        .args(["diff", "--name-status", "--no-renames"])
        .current_dir(root)
        .output()
        .ok()?;

    let mut results = Vec::new();
    if staged.status.success() {
        results.extend(parse_name_status(&String::from_utf8_lossy(&staged.stdout)));
    }
    if unstaged.status.success() {
        results.extend(parse_name_status(&String::from_utf8_lossy(
            &unstaged.stdout,
        )));
    }
    // Capture untracked files (new files not yet staged)
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(root)
        .output()
        .ok()?;
    if untracked.status.success() {
        for line in String::from_utf8_lossy(&untracked.stdout).lines() {
            let path = line.trim();
            if !path.is_empty() {
                results.push(('A', path.to_string()));
            }
        }
    }

    Some(results)
}

fn parse_name_status(output: &str) -> Vec<(char, String)> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let status = line.chars().next()?;
            let path = line[1..].trim().to_string();
            if path.is_empty() {
                return None;
            }
            Some((status, path))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn git_init(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    fn git_add_commit(dir: &Path, msg: &str) {
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", msg, "--allow-empty"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn test_head_oid_in_git_repo() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("test.txt"), "hello").unwrap();
        git_add_commit(dir.path(), "initial");

        let oid = head_oid(dir.path());
        assert!(oid.is_some());
        assert_eq!(oid.unwrap().len(), 40);
    }

    #[test]
    fn test_head_oid_not_git_repo() {
        let dir = TempDir::new().unwrap();
        assert!(head_oid(dir.path()).is_none());
    }

    #[test]
    fn test_detect_changes_with_modifications() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("file.rs"), "fn main() {}").unwrap();
        git_add_commit(dir.path(), "initial");

        let first_oid = head_oid(dir.path()).unwrap();

        fs::write(
            dir.path().join("file.rs"),
            "fn main() { println!(\"hi\"); }",
        )
        .unwrap();
        git_add_commit(dir.path(), "update");

        let diff = detect_changes(dir.path(), &first_oid).unwrap();
        assert!(diff.changed.contains(&"file.rs".to_string()));
        assert!(diff.deleted.is_empty());
    }

    #[test]
    fn test_detect_changes_with_deletion() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("keep.rs"), "keep").unwrap();
        fs::write(dir.path().join("remove.rs"), "remove").unwrap();
        git_add_commit(dir.path(), "initial");

        let first_oid = head_oid(dir.path()).unwrap();

        fs::remove_file(dir.path().join("remove.rs")).unwrap();
        git_add_commit(dir.path(), "delete");

        let diff = detect_changes(dir.path(), &first_oid).unwrap();
        assert!(diff.deleted.contains(&"remove.rs".to_string()));
    }

    #[test]
    fn test_detect_changes_no_changes() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("file.rs"), "content").unwrap();
        git_add_commit(dir.path(), "initial");

        let oid = head_oid(dir.path()).unwrap();

        let diff = detect_changes(dir.path(), &oid).unwrap();
        assert!(diff.changed.is_empty());
        assert!(diff.deleted.is_empty());
    }

    #[test]
    fn test_detect_uncommitted_changes() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("file.rs"), "original").unwrap();
        git_add_commit(dir.path(), "initial");

        let oid = head_oid(dir.path()).unwrap();

        fs::write(dir.path().join("file.rs"), "modified").unwrap();

        let diff = detect_changes(dir.path(), &oid).unwrap();
        assert!(diff.changed.contains(&"file.rs".to_string()));
    }

    #[test]
    fn test_detect_untracked_files() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("tracked.rs"), "content").unwrap();
        git_add_commit(dir.path(), "initial");

        let oid = head_oid(dir.path()).unwrap();

        // Create untracked file (not git add-ed)
        fs::write(dir.path().join("untracked.rs"), "new content").unwrap();

        let diff = detect_changes(dir.path(), &oid).unwrap();
        assert!(diff.changed.contains(&"untracked.rs".to_string()));
    }

    #[test]
    fn test_detect_changes_delete_then_recreate() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("file.rs"), "v1").unwrap();
        git_add_commit(dir.path(), "initial");

        let first_oid = head_oid(dir.path()).unwrap();

        // Delete and commit
        fs::remove_file(dir.path().join("file.rs")).unwrap();
        git_add_commit(dir.path(), "delete");

        // Recreate (untracked)
        fs::write(dir.path().join("file.rs"), "v2").unwrap();

        let diff = detect_changes(dir.path(), &first_oid).unwrap();
        // File should be in changed (recreated), NOT in deleted
        assert!(diff.changed.contains(&"file.rs".to_string()));
        assert!(!diff.deleted.contains(&"file.rs".to_string()));
    }

    #[test]
    fn test_detect_changes_invalid_commit() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("file.rs"), "content").unwrap();
        git_add_commit(dir.path(), "initial");

        let diff = detect_changes(dir.path(), "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef");
        assert!(diff.is_none());
    }

    #[test]
    fn test_parse_name_status() {
        let output = "M\tfile1.rs\nA\tfile2.rs\nD\tfile3.rs\n";
        let result = parse_name_status(output);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], ('M', "file1.rs".to_string()));
        assert_eq!(result[1], ('A', "file2.rs".to_string()));
        assert_eq!(result[2], ('D', "file3.rs".to_string()));
    }
}
