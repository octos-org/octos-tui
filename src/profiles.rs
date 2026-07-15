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

/// How a launch command resolves the server data dir.
#[derive(Debug, PartialEq, Eq)]
enum DataDirResolution {
    /// No `--data-dir` / `OCTOS_HOME=` override — the default `~/.octos` applies.
    None,
    /// An override we resolved to a concrete path.
    Resolved(PathBuf),
    /// An explicit override we could NOT resolve statically (e.g. a `$PWD`-style
    /// shell expression). We must NOT guess a default here — that would point at
    /// a DIFFERENT server's profiles.
    Unresolvable,
}

/// Resolve `<data_dir>/profiles` from the launch command. An explicit
/// `--data-dir` wins over an `OCTOS_HOME=` prefix, which wins over the
/// conventional `~/.octos`. Returns `None` (no profiles dir) when there is no
/// launch command, or when the command names an override we cannot resolve —
/// the latter degrades the picker to onboarding rather than offering profiles
/// from the wrong server.
pub fn solo_profiles_dir(stdio_command: Option<&str>) -> Option<PathBuf> {
    let data_dir = match stdio_command.map(data_dir_from_command) {
        // Explicit-but-unresolvable override: do NOT fall back to the default.
        Some(DataDirResolution::Unresolvable) => return None,
        Some(DataDirResolution::Resolved(dir)) => dir,
        // No override in the command → the conventional default.
        Some(DataDirResolution::None) => default_octos_home()?,
        // No launch command at all (remote/WebSocket launch).
        None => return None,
    };
    Some(data_dir.join("profiles"))
}

/// Parse the server data dir out of a launch command's tokens: `--data-dir
/// <path>` / `--data-dir=<path>`, else a leading `OCTOS_HOME=<path>` env
/// assignment. Distinguishes "no override present" ([`DataDirResolution::None`])
/// from "override present but unresolvable" ([`DataDirResolution::Unresolvable`])
/// so the caller can default only in the former case.
fn data_dir_from_command(command: &str) -> DataDirResolution {
    let tokens = shlex::split(command).unwrap_or_default();

    // `--data-dir <path>` or `--data-dir=<path>` (explicit flag wins).
    let mut iter = tokens.iter();
    while let Some(token) = iter.next() {
        if let Some(rest) = token.strip_prefix("--data-dir=") {
            return resolve_path_token(rest);
        }
        if token == "--data-dir" {
            return match iter.next() {
                Some(path) => resolve_path_token(path),
                // `--data-dir` with no value is a malformed but explicit
                // override — refuse to guess.
                None => DataDirResolution::Unresolvable,
            };
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

    DataDirResolution::None
}

/// Resolve a raw path token, expanding a leading `~`. A token carrying an
/// unresolved shell variable (`$…`) or an empty value is [`Unresolvable`] — an
/// explicit override we must not silently replace with the default.
fn resolve_path_token(raw: &str) -> DataDirResolution {
    let cleaned = raw.trim().trim_matches(['"', '\'']);
    if cleaned.is_empty() || cleaned.contains('$') {
        return DataDirResolution::Unresolvable;
    }
    DataDirResolution::Resolved(expand_tilde(cleaned))
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

/// Resolve the server data dir (the parent of `profiles/`) from the launch
/// command — the same resolution as [`solo_profiles_dir`], one level up. Used by
/// the in-TUI profiles surface to read the `default-profile` pointer and to do
/// set-default / delete on disk. `None` when it can't be resolved (remote launch
/// or an unresolvable `--data-dir`).
pub fn solo_data_dir(stdio_command: Option<&str>) -> Option<PathBuf> {
    solo_profiles_dir(stdio_command).and_then(|dir| dir.parent().map(Path::to_path_buf))
}

/// Read the `default-profile` pointer (a bare profile id), trimmed; `None` when
/// the file is absent or empty.
pub fn read_default_profile(data_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(data_dir.join("default-profile")).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// A one-line LLM summary for a profile (`family/model via route`), read only
/// from `config.llm.primary` — never `config.env_vars` (which holds secrets).
/// `None` when the descriptor is unreadable or has no primary LLM configured.
pub fn profile_llm_summary(profiles_dir: &Path, id: &str) -> Option<String> {
    let raw = std::fs::read_to_string(profiles_dir.join(format!("{id}.json"))).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let primary = value.get("config")?.get("llm")?.get("primary")?;
    let family = primary
        .get("family_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let model = primary
        .get("model_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let mut detail = format!("{family}/{model}");
    if let Some(route) = primary
        .get("route")
        .and_then(|route| route.get("route_id"))
        .and_then(serde_json::Value::as_str)
    {
        detail.push_str(&format!(" via {route}"));
    }
    Some(detail)
}

/// Set an existing profile as the machine default by atomically writing the
/// `default-profile` pointer (temp file + rename, so a crash never tears it).
pub fn set_default_profile(data_dir: &Path, id: &str) -> std::io::Result<()> {
    let pointer = data_dir.join("default-profile");
    let tmp = data_dir.join(".default-profile.tmp");
    std::fs::write(&tmp, id.as_bytes())?;
    std::fs::rename(&tmp, &pointer)
}

/// Delete a profile: its `<id>.json` descriptor and its `<id>/` data dir (which
/// holds that profile's sessions/episodes). If it was the machine default, the
/// pointer is cleared so nothing points at a ghost. Missing pieces are ignored
/// (idempotent); a real removal error propagates.
pub fn delete_profile(data_dir: &Path, id: &str) -> std::io::Result<()> {
    let profiles_dir = data_dir.join("profiles");
    let json = profiles_dir.join(format!("{id}.json"));
    match std::fs::remove_file(&json) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let dir = profiles_dir.join(id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir)?;
    }
    if read_default_profile(data_dir).as_deref() == Some(id) {
        let _ = std::fs::remove_file(data_dir.join("default-profile"));
    }
    Ok(())
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
        assert_eq!(
            data_dir_from_command("octos serve --stdio --solo --data-dir /srv/octos"),
            DataDirResolution::Resolved(PathBuf::from("/srv/octos"))
        );
    }

    #[test]
    fn should_read_data_dir_from_equals_form() {
        assert_eq!(
            data_dir_from_command("octos serve --data-dir=/srv/eq"),
            DataDirResolution::Resolved(PathBuf::from("/srv/eq"))
        );
    }

    #[test]
    fn should_read_data_dir_from_octos_home_env_prefix() {
        assert_eq!(
            data_dir_from_command("OCTOS_HOME=/srv/home octos serve --stdio"),
            DataDirResolution::Resolved(PathBuf::from("/srv/home"))
        );
    }

    #[test]
    fn should_report_none_when_no_override_present() {
        // No `--data-dir` / `OCTOS_HOME=` → default `~/.octos` is correct.
        assert_eq!(
            data_dir_from_command("octos serve --stdio --solo"),
            DataDirResolution::None
        );
    }

    #[test]
    fn should_report_unresolvable_for_shell_expression_override() {
        // An explicit-but-dynamic override must be flagged unresolvable, NOT
        // silently defaulted (that would scan a DIFFERENT server's profiles).
        assert_eq!(
            data_dir_from_command("OCTOS_HOME=\"$PWD/.octos\" octos serve"),
            DataDirResolution::Unresolvable
        );
        assert_eq!(
            data_dir_from_command("octos serve --data-dir $HOME/x"),
            DataDirResolution::Unresolvable
        );
    }

    #[test]
    fn solo_profiles_dir_returns_none_for_unresolvable_override() {
        // The whole point of the P2 fix: an unresolvable override degrades to
        // "no profiles" (→ onboarding) instead of falling back to the default
        // dir and offering foreign profiles.
        assert!(solo_profiles_dir(Some("OCTOS_HOME=$PWD/.octos octos serve --stdio")).is_none());
        // No command at all (remote launch) also yields no local profiles dir.
        assert!(solo_profiles_dir(None).is_none());
    }

    #[test]
    fn solo_profiles_dir_defaults_only_when_no_override() {
        // No override → default `~/.octos/profiles` (ends with the expected tail).
        let dir = solo_profiles_dir(Some("octos serve --stdio --solo"));
        // Only assert structure when a home dir is resolvable in the test env.
        if let Some(dir) = dir {
            assert!(dir.ends_with("profiles"));
            assert!(dir.to_string_lossy().contains(".octos"));
        }
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

    /// Seed a data dir with `<id>.json` + `<id>/` per profile and an optional
    /// default pointer.
    fn seed_data_dir(tag: &str, profiles: &[&str], default: Option<&str>) -> TempDir {
        let tmp = TempDir::new(tag);
        let profiles_dir = tmp.path().join("profiles");
        fs::create_dir_all(&profiles_dir).unwrap();
        for id in profiles {
            fs::write(profiles_dir.join(format!("{id}.json")), "{}").unwrap();
            fs::create_dir_all(profiles_dir.join(id)).unwrap();
        }
        if let Some(default) = default {
            fs::write(tmp.path().join("default-profile"), default).unwrap();
        }
        tmp
    }

    #[test]
    fn set_default_profile_writes_the_pointer() {
        let tmp = seed_data_dir("set-default", &["glm", "work"], None);
        set_default_profile(tmp.path(), "work").unwrap();
        assert_eq!(read_default_profile(tmp.path()).as_deref(), Some("work"));
        // Overwriting is fine.
        set_default_profile(tmp.path(), "glm").unwrap();
        assert_eq!(read_default_profile(tmp.path()).as_deref(), Some("glm"));
    }

    #[test]
    fn delete_profile_removes_descriptor_data_and_clears_default() {
        let tmp = seed_data_dir("delete-default", &["glm", "work"], Some("work"));
        delete_profile(tmp.path(), "work").unwrap();
        let profiles_dir = tmp.path().join("profiles");
        assert!(!profiles_dir.join("work.json").exists());
        assert!(!profiles_dir.join("work").exists());
        assert!(profiles_dir.join("glm.json").exists(), "others untouched");
        assert!(
            read_default_profile(tmp.path()).is_none(),
            "deleting the default clears the pointer"
        );
    }

    #[test]
    fn delete_a_non_default_profile_keeps_the_pointer() {
        let tmp = seed_data_dir("delete-nondefault", &["glm", "work"], Some("glm"));
        delete_profile(tmp.path(), "work").unwrap();
        assert_eq!(read_default_profile(tmp.path()).as_deref(), Some("glm"));
    }

    #[test]
    fn profile_llm_summary_reads_primary_without_secrets() {
        let tmp = seed_data_dir("summary", &[], None);
        let profiles_dir = tmp.path().join("profiles");
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(
            profiles_dir.join("glm.json"),
            r#"{"config":{"llm":{"primary":{"family_id":"zai","model_id":"glm-5.2","route":{"route_id":"zai"}}},"env_vars":{"ZAI_API_KEY":"sk-secret"}}}"#,
        )
        .unwrap();
        let summary = profile_llm_summary(&profiles_dir, "glm").expect("summary");
        assert_eq!(summary, "zai/glm-5.2 via zai");
        assert!(!summary.contains("sk-secret"), "never leaks the API key");
    }

    #[test]
    fn solo_data_dir_is_the_parent_of_the_profiles_dir() {
        let command = "octos serve --stdio --solo --data-dir /tmp/xyz";
        assert_eq!(
            solo_data_dir(Some(command)),
            Some(PathBuf::from("/tmp/xyz"))
        );
    }
}
