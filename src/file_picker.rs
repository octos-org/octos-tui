//! `@` composer file picker (#363, v1: path insert only).
//!
//! Scans the ACTIVE workspace root on the machine octos-tui runs on and feeds
//! the searchable `file-picker` menu (see `menu::providers::file_picker_menu`).
//! Selecting a row inserts the file's RELATIVE path at the composer cursor —
//! no file contents are read or embedded, and no protocol traffic is involved.
//!
//! The walk is breadth-first (shallow paths surface first, which is what a
//! picker wants), sorted per directory for deterministic ordering, and bounded
//! by [`FILE_PICKER_MAX_FILES`] / [`FILE_PICKER_MAX_DEPTH`] plus a directory
//! visit budget so a pathological tree can never stall the render loop. `.git`
//! and the ubiquitous dependency/build trees (`target`, `node_modules`) are
//! skipped — with a ~2000-entry cap they would otherwise flood out the source
//! files the picker exists to find. Symlinks are skipped entirely (no cycle
//! chasing, no walking out of the workspace).

use std::collections::VecDeque;
use std::path::Path;

/// Maximum number of files the picker lists (the "~2000 entries" cap).
pub const FILE_PICKER_MAX_FILES: usize = 2000;
/// Maximum directory depth below the workspace root that is walked.
pub const FILE_PICKER_MAX_DEPTH: usize = 16;
/// Budget of directories visited per scan, so a wide tree of empty/filtered
/// directories still terminates promptly even before the file cap is hit.
const FILE_PICKER_MAX_DIRS: usize = 2000;

/// Directory names that are never descended into. `.git` per the picker spec;
/// `target` / `node_modules` because either one routinely holds tens of
/// thousands of generated files that would exhaust the entry cap before the
/// project's real sources are reached.
const SKIPPED_DIRS: [&str; 3] = [".git", "target", "node_modules"];

/// Snapshot of one `@` picker opening: the root that was scanned (display
/// form) and the relative paths found under it. Rebuilt on every open — the
/// picker never serves a stale tree from an earlier `@`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePickerState {
    /// Display form of the scanned workspace root.
    pub root: String,
    /// Relative paths (always `/`-separated) in breadth-first, per-directory
    /// alphabetical order.
    pub files: Vec<String>,
    /// Whether any cap (files / depth / directory budget) truncated the scan.
    pub truncated: bool,
}

impl FilePickerState {
    /// Scan `root` and build the picker snapshot.
    pub fn scan(root: &Path) -> Self {
        let (files, truncated) = scan_workspace_files(root);
        Self {
            root: root.display().to_string(),
            files,
            truncated,
        }
    }
}

/// Walk `root` breadth-first and return `(relative_paths, truncated)`.
///
/// Paths are `/`-joined relative to `root` regardless of platform so the
/// inserted prompt text is stable. Entries within each directory are sorted by
/// name; directories in [`SKIPPED_DIRS`] and all symlinks are skipped.
pub fn scan_workspace_files(root: &Path) -> (Vec<String>, bool) {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut dirs_visited = 0usize;
    // Queue of (absolute dir, relative prefix, depth). Depth 0 = root itself.
    let mut queue = VecDeque::from([(root.to_path_buf(), String::new(), 0usize)]);

    'walk: while let Some((dir, rel_prefix, depth)) = queue.pop_front() {
        if dirs_visited >= FILE_PICKER_MAX_DIRS {
            truncated = true;
            break;
        }
        dirs_visited += 1;

        let Ok(entries) = std::fs::read_dir(&dir) else {
            // Unreadable directory (permissions, raced deletion): skip quietly.
            continue;
        };
        let mut children: Vec<(String, bool)> = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file_type = entry.file_type().ok()?;
                // `DirEntry::file_type` does not follow symlinks, so a symlink
                // reports `is_symlink` and is dropped here — never followed.
                if file_type.is_symlink() {
                    return None;
                }
                let name = entry.file_name().into_string().ok()?;
                Some((name, file_type.is_dir()))
            })
            .collect();
        children.sort();

        for (name, is_dir) in children {
            let rel = if rel_prefix.is_empty() {
                name.clone()
            } else {
                format!("{rel_prefix}/{name}")
            };
            if is_dir {
                if SKIPPED_DIRS.contains(&name.as_str()) {
                    continue;
                }
                if depth + 1 > FILE_PICKER_MAX_DEPTH {
                    truncated = true;
                    continue;
                }
                queue.push_back((dir.join(&name), rel, depth + 1));
            } else {
                if files.len() >= FILE_PICKER_MAX_FILES {
                    truncated = true;
                    break 'walk;
                }
                files.push(rel);
            }
        }
    }

    (files, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Minimal self-cleaning temp dir (repo convention — no tempfile dep).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "octos-tui-file-picker-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(&path, b"x").expect("write file");
    }

    #[test]
    fn scan_lists_relative_paths_breadth_first_and_sorted() {
        let dir = TempDir::new("order");
        touch(dir.path(), "b.rs");
        touch(dir.path(), "a.rs");
        touch(dir.path(), "sub/inner.rs");

        let (files, truncated) = scan_workspace_files(dir.path());

        assert_eq!(files, vec!["a.rs", "b.rs", "sub/inner.rs"]);
        assert!(!truncated);
    }

    #[test]
    fn scan_skips_git_target_and_node_modules() {
        let dir = TempDir::new("skips");
        touch(dir.path(), "keep.rs");
        touch(dir.path(), ".git/config");
        touch(dir.path(), "target/debug/artifact");
        touch(dir.path(), "node_modules/pkg/index.js");
        // A nested `.git` (submodule/worktree layout) is skipped too.
        touch(dir.path(), "sub/.git/HEAD");
        touch(dir.path(), "sub/kept.txt");

        let (files, _) = scan_workspace_files(dir.path());

        assert_eq!(files, vec!["keep.rs", "sub/kept.txt"]);
    }

    #[test]
    fn scan_includes_dotfiles_but_not_symlinks() {
        let dir = TempDir::new("dotfiles");
        touch(dir.path(), ".gitignore");
        #[cfg(unix)]
        {
            touch(dir.path(), "real.txt");
            std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("link.txt"))
                .expect("create symlink");
        }

        let (files, _) = scan_workspace_files(dir.path());

        assert!(files.contains(&".gitignore".to_string()));
        #[cfg(unix)]
        {
            assert!(files.contains(&"real.txt".to_string()));
            assert!(!files.contains(&"link.txt".to_string()));
        }
    }

    #[test]
    fn scan_truncates_at_file_cap() {
        let dir = TempDir::new("cap");
        for i in 0..(FILE_PICKER_MAX_FILES + 5) {
            touch(dir.path(), &format!("f{i:05}.txt"));
        }

        let (files, truncated) = scan_workspace_files(dir.path());

        assert_eq!(files.len(), FILE_PICKER_MAX_FILES);
        assert!(truncated);
    }

    #[test]
    fn scan_missing_root_is_empty_not_error() {
        let dir = TempDir::new("gone");
        let missing = dir.path().join("does-not-exist");

        let (files, truncated) = scan_workspace_files(&missing);

        assert!(files.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn state_scan_records_display_root() {
        let dir = TempDir::new("state");
        touch(dir.path(), "x.txt");

        let state = FilePickerState::scan(dir.path());

        assert_eq!(state.root, dir.path().display().to_string());
        assert_eq!(state.files, vec!["x.txt"]);
        assert!(!state.truncated);
    }
}
