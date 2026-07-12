//! Phase 3 startup profile discovery.
//!
//! octos-tui has no UI-protocol `profile/list` method for account/local
//! profiles, so for the solo-local use case it discovers existing profiles
//! straight from the server's on-disk layout: `<data_dir>/profiles/<id>.json`.
//! The data dir is resolved from the launch command — an explicit
//! `--data-dir <path>` flag or an `OCTOS_HOME=<path>` env prefix — falling back
//! to the conventional `~/.octos`.
//!
//! Every step is best-effort: any failure (no home dir, unreadable dir, a
//! `$PWD`-style dynamic path we can't resolve) yields an empty list. The
//! startup [`crate::model::StartupProfileDecision`] treats an empty list as "no
//! profiles" and runs onboarding, so a wrong guess never blocks launch.

use std::path::{Path, PathBuf};

/// Discover local profile ids from the launch command's data dir. Returns a
/// sorted, de-duplicated list; empty when the dir cannot be resolved or read.
pub fn discover_local_profile_ids(stdio_command: Option<&str>) -> Vec<String> {
    solo_profiles_dir(stdio_command)
        .map(|dir| enumerate_profile_ids(&dir))
        .unwrap_or_default()
}

/// Resolve `<data_dir>/profiles` from the launch command. An explicit
/// `--data-dir` wins over an `OCTOS_HOME=` prefix, which wins over the
/// conventional `~/.octos`.
pub fn solo_profiles_dir(stdio_command: Option<&str>) -> Option<PathBuf> {
    let data_dir = stdio_command
        .and_then(data_dir_from_command)
        .or_else(default_octos_home)?;
    Some(data_dir.join("profiles"))
}

/// Parse the server data dir out of a launch command's tokens: `--data-dir
/// <path>` / `--data-dir=<path>`, else a leading `OCTOS_HOME=<path>` env
/// assignment. Returns `None` when neither is present or the value is a shell
/// expression we cannot resolve statically (e.g. `$PWD/.octos`).
fn data_dir_from_command(command: &str) -> Option<PathBuf> {
    let tokens = shlex::split(command).unwrap_or_default();

    // `--data-dir <path>` or `--data-dir=<path>` (explicit flag wins).
    let mut iter = tokens.iter();
    while let Some(token) = iter.next() {
        if let Some(rest) = token.strip_prefix("--data-dir=") {
            return resolve_path_token(rest);
        }
        if token == "--data-dir" {
            if let Some(path) = iter.next() {
                return resolve_path_token(path);
            }
        }
    }

    // Leading `OCTOS_HOME=<path>` env assignment (before the program token).
    for token in &tokens {
        if let Some(rest) = token.strip_prefix("OCTOS_HOME=") {
            return resolve_path_token(rest);
        }
        // Env assignments only precede the program; stop at the first plain
        // (non `KEY=VALUE`) token so we do not mistake a later `--flag=value`.
        if !token.contains('=') {
            break;
        }
    }

    None
}

/// Turn a raw path token into a usable [`PathBuf`], expanding a leading `~`.
/// Rejects tokens carrying an unresolved shell variable (`$…`), which we cannot
/// evaluate here — callers fall back to the default in that case.
fn resolve_path_token(raw: &str) -> Option<PathBuf> {
    let cleaned = raw.trim().trim_matches(['"', '\'']);
    if cleaned.is_empty() || cleaned.contains('$') {
        return None;
    }
    Some(expand_tilde(cleaned))
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// List profile ids in a `profiles` directory: the file stem of every
/// `<id>.json` descriptor, sorted and de-duplicated. Empty when the directory
/// is absent or unreadable.
pub fn enumerate_profile_ids(profiles_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(profiles_dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            // Profile descriptors are `<id>.json` files; ignore anything else
            // (per-profile `data/` subdirs, lockfiles, hidden files, …).
            if !path.is_file() {
                return None;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                return None;
            }
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
                .filter(|id| !id.is_empty() && !id.starts_with('.'))
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn default_octos_home() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".octos"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|home| !home.is_empty())
                .map(PathBuf::from)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A unique temp dir for a test, cleaned up on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut dir = std::env::temp_dir();
            let unique = format!(
                "octos-tui-profiles-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            dir.push(unique);
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

    #[test]
    fn should_read_data_dir_from_data_dir_flag() {
        let dir = data_dir_from_command("octos serve --stdio --solo --data-dir /srv/octos")
            .expect("data-dir parsed");
        assert_eq!(dir, PathBuf::from("/srv/octos"));
    }

    #[test]
    fn should_read_data_dir_from_equals_form() {
        let dir = data_dir_from_command("octos serve --data-dir=/srv/eq").expect("parsed");
        assert_eq!(dir, PathBuf::from("/srv/eq"));
    }

    #[test]
    fn should_read_data_dir_from_octos_home_env_prefix() {
        let dir =
            data_dir_from_command("OCTOS_HOME=/srv/home octos serve --stdio").expect("parsed");
        assert_eq!(dir, PathBuf::from("/srv/home"));
    }

    #[test]
    fn should_reject_unresolvable_shell_path() {
        // `$PWD/.octos` cannot be resolved statically → None (caller defaults).
        assert!(data_dir_from_command("OCTOS_HOME=\"$PWD/.octos\" octos serve").is_none());
        assert!(data_dir_from_command("octos serve").is_none());
    }

    #[test]
    fn should_enumerate_only_json_profile_stems_sorted() {
        let tmp = TempDir::new("enum");
        fs::write(tmp.path().join("glm.json"), "{}").unwrap();
        fs::write(tmp.path().join("deepseek.json"), "{}").unwrap();
        // Non-profile entries are ignored.
        fs::write(tmp.path().join("notes.txt"), "x").unwrap();
        fs::create_dir_all(tmp.path().join("glm")).unwrap();

        let ids = enumerate_profile_ids(tmp.path());
        assert_eq!(ids, vec!["deepseek".to_owned(), "glm".to_owned()]);
    }

    #[test]
    fn should_return_empty_when_profiles_dir_missing() {
        let tmp = TempDir::new("missing");
        let ghost = tmp.path().join("does-not-exist");
        assert!(enumerate_profile_ids(&ghost).is_empty());
    }

    #[test]
    fn should_discover_end_to_end_via_data_dir_flag() {
        let tmp = TempDir::new("e2e");
        let profiles = tmp.path().join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(profiles.join("glm.json"), "{}").unwrap();
        fs::write(profiles.join("openai.json"), "{}").unwrap();

        let command = format!(
            "octos serve --stdio --solo --data-dir {}",
            tmp.path().display()
        );
        let ids = discover_local_profile_ids(Some(&command));
        assert_eq!(ids, vec!["glm".to_owned(), "openai".to_owned()]);
    }
}
