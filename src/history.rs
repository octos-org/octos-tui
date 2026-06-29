//! Composer command-history navigation (codex / claude-code style).
//!
//! Up/Down at the composer edges recall previously submitted prompts. The
//! recall gate is deliberately simple and robust against this crate's
//! per-session draft model:
//!
//! * Browsing is only **entered from an empty composer**, so a non-empty draft
//!   keeps its ordinary first-line transcript-scroll affordance.
//! * Browsing only **continues while the composer still equals the last
//!   recalled entry**. That single equality check doubles as edit-invalidation:
//!   the moment the user edits a recalled entry it stops matching, so the next
//!   Up/Down falls through to cursor movement / scroll instead of clobbering
//!   the edit. No live-draft "stash" is kept — a global stash would collide
//!   with this crate's per-session drafts on a session switch.
//!
//! Entries are global across sessions and persisted to
//! `~/.config/octos-tui/history.jsonl` (one JSON-encoded string per line,
//! newest last), mirroring the TUI config-dir convention in
//! [`crate::cli::default_config_path`]. Persistence is **opt-in via
//! [`ComposerHistory::persist_path`]**: it is `None` by default (and after
//! [`Default`]/[`ComposerHistory::from_entries`]), so unit tests and any
//! pre-load state never touch real disk. [`ComposerHistory::load`] sets it.
//! All disk I/O is best-effort: a failed read/write degrades to in-memory and
//! never interrupts the session.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Maximum entries kept in memory and trimmed-to on load (oldest dropped).
const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposerHistory {
    /// Submitted prompts, oldest → newest. Global across sessions.
    entries: Vec<String>,
    /// `Some(i)` while the user is browsing `entries[i]`; `None` on the live
    /// draft. Reset on submit, edit-away, session switch, and snapshot replay.
    nav_index: Option<usize>,
    /// When `Some`, newly recorded entries are appended here (best-effort).
    /// `None` keeps the ring purely in-memory (default / tests).
    persist_path: Option<PathBuf>,
}

impl ComposerHistory {
    /// Build an in-memory history from a known entry list (newest last). No
    /// persistence path — used by tests and as a building block.
    pub fn from_entries(mut entries: Vec<String>) -> Self {
        if entries.len() > MAX_ENTRIES {
            let excess = entries.len() - MAX_ENTRIES;
            entries.drain(0..excess);
        }
        Self {
            entries,
            nav_index: None,
            persist_path: None,
        }
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn is_navigating(&self) -> bool {
        self.nav_index.is_some()
    }

    /// Clear browsing state while keeping entries. Call on session switch,
    /// snapshot replay, and composer clear so a stale index never leaks into a
    /// different context.
    pub fn reset_navigation(&mut self) {
        self.nav_index = None;
    }

    /// Record a just-submitted prompt: trims, skips empties, dedups against the
    /// most recent entry, caps to `MAX_ENTRIES`, ends any browsing, and (when a
    /// `persist_path` is set) appends the new line to disk. Returns `true` when
    /// a new entry was appended.
    pub fn record(&mut self, prompt: &str) -> bool {
        self.nav_index = None;
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return false;
        }
        if self.entries.last().map(String::as_str) == Some(trimmed) {
            return false; // dedup consecutive duplicates
        }
        self.entries.push(trimmed.to_string());
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(0..excess);
        }
        if let Some(path) = self.persist_path.clone() {
            let _ = append_entry(&path, trimmed);
        }
        true
    }

    /// Up: recall an older entry. `current` is the live composer text. Returns
    /// the text to install, or `None` to fall through to the caller's
    /// cursor-move / transcript-scroll behavior.
    pub fn recall_prev(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        // From an empty composer, (re)start the browse at the newest entry —
        // whether or not we were already navigating. This also covers a recalled
        // entry the user cleared with Backspace/Delete (not just Ctrl+U), which
        // would otherwise need a second Up.
        if current.trim().is_empty() {
            let idx = self.entries.len() - 1;
            self.nav_index = Some(idx);
            return Some(self.entries[idx].clone());
        }
        match self.nav_index {
            // Non-empty draft, not browsing → leave it (caller scrolls).
            None => None,
            // Continue only while the composer still equals the recalled entry;
            // otherwise the user edited it — stop browsing and fall through.
            Some(idx) => {
                if self.entries.get(idx).map(String::as_str) != Some(current) {
                    self.nav_index = None;
                    return None;
                }
                let next = idx.saturating_sub(1); // stays put at the oldest entry
                self.nav_index = Some(next);
                Some(self.entries[next].clone())
            }
        }
    }

    /// Down: recall a newer entry, or return to an empty draft past the newest.
    /// Returns `None` when not browsing or after an edit-away, so Down keeps its
    /// transcript-scroll behavior.
    pub fn recall_next(&mut self, current: &str) -> Option<String> {
        let idx = self.nav_index?;
        if self.entries.get(idx).map(String::as_str) != Some(current) {
            self.nav_index = None;
            return None;
        }
        if idx + 1 < self.entries.len() {
            self.nav_index = Some(idx + 1);
            Some(self.entries[idx + 1].clone())
        } else {
            self.nav_index = None;
            Some(String::new()) // past the newest → back to an empty draft
        }
    }

    // ---- persistence (best-effort; never panics, never blocks a turn) ----

    /// `~/.config/octos-tui/history.jsonl` (HOME, then USERPROFILE on Windows),
    /// mirroring [`crate::cli::default_config_path`].
    pub fn default_path() -> Option<PathBuf> {
        history_path_from_home(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
    }

    /// Load from the default path and bind it for future appends. Missing file
    /// or unparseable lines degrade to whatever parsed (still bound, so the
    /// first submit creates the file).
    pub fn load_from_default_path() -> Self {
        match Self::default_path() {
            Some(path) => Self::load(&path),
            None => Self::default(),
        }
    }

    /// Load from `path` (one JSON-encoded string per line, newest last) and
    /// bind `path` as the persistence target for subsequent [`Self::record`].
    pub fn load(path: &Path) -> Self {
        let mut entries = Vec::new();
        if let Ok(contents) = std::fs::read_to_string(path) {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<String>(line) {
                    if !entry.trim().is_empty() {
                        entries.push(entry);
                    }
                }
            }
        }
        // Bound the on-disk file: appends are otherwise unbounded, so whenever a
        // load sees more than the cap, compact back down to it (rewrite below).
        let oversized = entries.len() > MAX_ENTRIES;
        if oversized {
            let excess = entries.len() - MAX_ENTRIES;
            entries.drain(0..excess);
        }
        // A pre-existing history may carry looser permissions (created before
        // this feature, or by another tool); tighten it on load so stored
        // prompts are owner-only immediately, not only after the next append.
        tighten_permissions(path);
        if oversized {
            let _ = rewrite_history_file(path, &entries);
        }
        Self {
            entries,
            nav_index: None,
            persist_path: Some(path.to_path_buf()),
        }
    }
}

/// Overwrite `path` with exactly `entries` (one JSON line each), owner-only,
/// via a temp sibling + atomic rename so a crash mid-write can't corrupt the
/// existing history. Best-effort: a failure leaves the current file intact.
fn rewrite_history_file(path: &Path, entries: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = String::new();
    for entry in entries {
        body.push_str(&serde_json::to_string(entry).unwrap_or_default());
        body.push('\n');
    }
    let tmp = path.with_extension("jsonl.compact");
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        let mut file = opts.open(&tmp)?;
        file.write_all(body.as_bytes())?;
    }
    // `mode(0o600)` above is ignored if a stale temp already existed; force
    // owner-only before the rename publishes it as the history file.
    tighten_permissions(&tmp);
    std::fs::rename(&tmp, path)
}

/// Pure resolver: prefer `HOME`, fall back to `USERPROFILE` (Windows). Empty
/// values are ignored. Split out so it is testable without mutating process env
/// (`std::env::set_var` is `unsafe` under edition 2024 + `unsafe_code = deny`).
fn history_path_from_home(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let base = home
        .filter(|value| !value.is_empty())
        .or_else(|| userprofile.filter(|value| !value.is_empty()))?;
    Some(
        PathBuf::from(base)
            .join(".config")
            .join("octos-tui")
            .join("history.jsonl"),
    )
}

/// Append a single JSON-encoded line, creating the dir/file with owner-only
/// (`0600`) permissions on Unix. Plaintext history can contain secrets the user
/// pasted, so keep it unreadable by other users. The JSON line + trailing
/// newline are written in ONE `write_all`, which is atomic with `O_APPEND` for
/// the small lines we write (avoids interleaved/torn lines when more than one
/// `octos-tui` shares the file).
fn append_entry(path: &Path, entry: &str) -> std::io::Result<()> {
    let trimmed = entry.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_string(trimmed)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    // `mode(0o600)` above only applies when CREATING the file; tighten a
    // pre-existing history too (shared with `load`) so it stays owner-only.
    tighten_permissions(path);
    let mut file = opts.open(path)?;
    file.write_all(line.as_bytes())
}

/// Best-effort: ensure `path`, when it exists, is owner-only (`0600`) on Unix.
/// No-op on non-Unix, when the file is absent, or when it is already `0600`.
/// Plaintext history can hold pasted secrets, so it must never be group/world
/// readable — and `OpenOptions::mode` only governs newly created files.
fn tighten_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            if perms.mode() & 0o777 != 0o600 {
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn h(items: &[&str]) -> ComposerHistory {
        ComposerHistory::from_entries(items.iter().map(|s| s.to_string()).collect())
    }

    fn temp_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is valid")
            .as_nanos();
        std::env::temp_dir().join(format!("octos-tui-hist-{name}-{nonce}.jsonl"))
    }

    #[test]
    fn default_has_no_persist_path() {
        // Critical: tests and pre-load state must never write to real disk.
        assert!(ComposerHistory::default().persist_path.is_none());
        assert!(h(&["a"]).persist_path.is_none());
    }

    #[test]
    fn record_in_memory_does_not_touch_disk() {
        let mut hist = ComposerHistory::default();
        assert!(hist.record("first"));
        assert!(hist.record("second"));
        assert!(!hist.record("second")); // consecutive dup ignored
        assert!(hist.record("  third  ")); // trimmed
        assert_eq!(hist.entries(), &["first", "second", "third"]);
    }

    #[test]
    fn record_ignores_empty_and_resets_navigation() {
        let mut hist = h(&["a", "b"]);
        assert_eq!(hist.recall_prev(""), Some("b".to_string()));
        assert!(hist.is_navigating());
        assert!(!hist.record("   ")); // whitespace-only ⇒ no entry
        assert!(!hist.is_navigating()); // but navigation reset
        assert_eq!(hist.entries(), &["a", "b"]);
    }

    #[test]
    fn record_caps_at_max_entries() {
        let mut hist = ComposerHistory::default();
        for i in 0..(MAX_ENTRIES + 50) {
            hist.record(&format!("cmd{i}"));
        }
        assert_eq!(hist.entries().len(), MAX_ENTRIES);
        assert_eq!(hist.entries().first().map(String::as_str), Some("cmd50"));
        assert_eq!(
            hist.entries().last().map(String::as_str),
            Some(format!("cmd{}", MAX_ENTRIES + 49).as_str())
        );
    }

    #[test]
    fn recall_prev_from_empty_starts_at_newest_then_steps_older() {
        let mut hist = h(&["one", "two", "three"]);
        assert_eq!(hist.recall_prev(""), Some("three".to_string()));
        assert_eq!(hist.recall_prev("three"), Some("two".to_string()));
        assert_eq!(hist.recall_prev("two"), Some("one".to_string()));
    }

    #[test]
    fn recall_prev_stays_put_at_oldest() {
        let mut hist = h(&["one", "two"]);
        assert_eq!(hist.recall_prev(""), Some("two".to_string()));
        assert_eq!(hist.recall_prev("two"), Some("one".to_string()));
        // Already at the oldest: re-applies it, never scrolls past.
        assert_eq!(hist.recall_prev("one"), Some("one".to_string()));
        assert_eq!(hist.recall_prev("one"), Some("one".to_string()));
    }

    #[test]
    fn recall_prev_restarts_from_newest_after_composer_cleared() {
        let mut hist = h(&["one", "two", "three"]);
        assert_eq!(hist.recall_prev(""), Some("three".to_string()));
        assert_eq!(hist.recall_prev("three"), Some("two".to_string()));
        // User Backspace-clears the recalled entry: composer empty but still
        // mid-browse. Next Up restarts at the newest (not a no-op that scrolls).
        assert_eq!(hist.recall_prev(""), Some("three".to_string()));
    }

    #[test]
    fn recall_prev_from_nonempty_draft_falls_through() {
        let mut hist = h(&["one", "two"]);
        assert_eq!(hist.recall_prev("typing"), None); // gate: only from empty
        assert!(!hist.is_navigating());
    }

    #[test]
    fn recall_prev_returns_none_when_history_empty() {
        let mut hist = ComposerHistory::default();
        assert_eq!(hist.recall_prev(""), None);
    }

    #[test]
    fn recall_next_steps_newer_then_returns_empty_past_newest() {
        let mut hist = h(&["one", "two", "three"]);
        assert_eq!(hist.recall_prev(""), Some("three".to_string()));
        assert_eq!(hist.recall_prev("three"), Some("two".to_string()));
        assert_eq!(hist.recall_next("two"), Some("three".to_string()));
        // Past the newest ⇒ empty draft, navigation ends.
        assert_eq!(hist.recall_next("three"), Some(String::new()));
        assert!(!hist.is_navigating());
    }

    #[test]
    fn recall_next_returns_none_when_not_navigating() {
        let mut hist = h(&["one", "two"]);
        assert_eq!(hist.recall_next(""), None); // Down with no active browse ⇒ scroll
    }

    #[test]
    fn editing_recalled_entry_stops_browsing() {
        let mut hist = h(&["one", "two"]);
        assert_eq!(hist.recall_prev(""), Some("two".to_string()));
        // User edited "two" → "twoX": equality gate fails, browsing ends, both
        // directions fall through.
        assert_eq!(hist.recall_prev("twoX"), None);
        assert!(!hist.is_navigating());
        assert_eq!(hist.recall_next("twoX"), None);
    }

    #[test]
    fn reset_navigation_keeps_entries() {
        let mut hist = h(&["a", "b"]);
        assert!(hist.recall_prev("").is_some());
        hist.reset_navigation();
        assert!(!hist.is_navigating());
        assert_eq!(hist.entries(), &["a", "b"]);
    }

    #[test]
    fn load_parses_jsonl_skips_blank_and_bad_lines_and_binds_path() {
        let path = temp_path("load");
        std::fs::write(
            &path,
            "\"hello\"\n\n\"multi\\nline\"\nnot-json\n\"world\"\n",
        )
        .unwrap();
        let hist = ComposerHistory::load(&path);
        assert_eq!(hist.entries(), &["hello", "multi\nline", "world"]);
        assert_eq!(hist.persist_path.as_deref(), Some(path.as_path()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_after_load_persists_and_reloads_in_order() {
        let path = temp_path("persist");
        let _ = std::fs::remove_file(&path);
        let mut hist = ComposerHistory::load(&path); // empty, path bound
        assert!(hist.record("first"));
        assert!(hist.record("second / with slash"));
        assert!(!hist.record("   ")); // whitespace skipped, not persisted
        let reloaded = ComposerHistory::load(&path);
        assert_eq!(reloaded.entries(), &["first", "second / with slash"]);
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn append_creates_owner_only_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("perms");
        let _ = std::fs::remove_file(&path);
        append_entry(&path, "secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn append_tightens_preexisting_loose_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("loose-perms");
        // Pre-create a world/group-readable history file (the gap codex flagged).
        std::fs::write(&path, "\"old\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        append_entry(&path, "new-secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "pre-existing file must be tightened");
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn load_tightens_preexisting_loose_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("load-perms");
        std::fs::write(&path, "\"x\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = ComposerHistory::load(&path); // load alone must tighten, pre-append
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "load must tighten an existing file");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_compacts_oversized_file_back_to_cap() {
        let path = temp_path("compact");
        let _ = std::fs::remove_file(&path);
        let mut body = String::new();
        for i in 0..(MAX_ENTRIES + 100) {
            body.push_str(&format!("\"cmd{i}\"\n"));
        }
        std::fs::write(&path, body).unwrap();
        let hist = ComposerHistory::load(&path);
        assert_eq!(hist.entries().len(), MAX_ENTRIES);
        // The file itself is rewritten down to the cap (bounds disk growth).
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, MAX_ENTRIES);
        assert_eq!(
            hist.entries().last().map(String::as_str),
            Some(format!("cmd{}", MAX_ENTRIES + 99).as_str())
        );
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn compaction_is_owner_only_even_with_stale_loose_temp() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("compact-perms");
        let tmp = path.with_extension("jsonl.compact");
        let _ = std::fs::remove_file(&path);
        let mut body = String::new();
        for i in 0..(MAX_ENTRIES + 5) {
            body.push_str(&format!("\"c{i}\"\n"));
        }
        std::fs::write(&path, body).unwrap();
        // Pre-existing world-readable temp (the stale-temp scenario).
        std::fs::write(&tmp, "\"stale\"\n").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = ComposerHistory::load(&path); // compacts via the temp + rename
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "compacted history must be owner-only");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn history_path_prefers_home_then_userprofile() {
        let home = history_path_from_home(Some("/home/u".into()), Some("C:\\u".into())).unwrap();
        assert!(home.ends_with(".config/octos-tui/history.jsonl"));
        assert!(home.starts_with("/home/u"));
        let win = history_path_from_home(None, Some("C:\\u".into())).unwrap();
        assert!(win.starts_with("C:\\u"));
        assert_eq!(history_path_from_home(None, None), None);
        assert_eq!(
            history_path_from_home(Some("".into()), Some("".into())),
            None
        );
    }
}
