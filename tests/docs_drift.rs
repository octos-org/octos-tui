//! Fails when `docs/ARCHITECTURE.md` falls behind the code it documents.
//!
//! Both tables in that file were hand-maintained and both had rotted: the
//! command table listed 7 of 82 `AppUiCommand` variants under the heading
//! "the stable command surface currently includes", and the layers table
//! listed 7 of 43 source files. Whole subsystems — `src/menu/`, `src/cmd/`,
//! `backend_ensure.rs`, `autonomy.rs` — were absent.
//!
//! Rather than ask reviewers to notice, these tests read the source and fail
//! the build. They parse text instead of using the types, deliberately: a
//! variant cannot be constructed without its params, and walking the file is
//! what keeps this test cheap enough to run everywhere.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Variant names of `pub enum AppUiCommand` in `src/model.rs`.
fn appui_command_variants(model_rs: &str) -> BTreeSet<String> {
    let start = model_rs
        .find("pub enum AppUiCommand")
        .expect("src/model.rs must declare `pub enum AppUiCommand`");
    let body = &model_rs[start..];
    let end = body
        .find("\n}")
        .expect("`pub enum AppUiCommand` must be brace-terminated");
    body[..end]
        .lines()
        .skip(1)
        .filter_map(|line| {
            // Variant lines are indented exactly one level inside the enum.
            let name = line.strip_prefix("    ")?;
            if name.starts_with(' ') || name.starts_with("//") || name.starts_with('#') {
                return None;
            }
            let ident: String = name
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let first = ident.chars().next()?;
            first.is_ascii_uppercase().then_some(ident)
        })
        .collect()
}

#[test]
fn architecture_documents_every_appui_command() {
    let variants = appui_command_variants(&read("src/model.rs"));
    assert!(
        variants.len() > 50,
        "parser found only {} variants — the enum shape changed and this test \
         is no longer reading it correctly",
        variants.len()
    );

    let doc = read("docs/ARCHITECTURE.md");
    let missing: Vec<&String> = variants
        .iter()
        .filter(|v| !doc.contains(&format!("`{v}`")))
        .collect();

    assert!(
        missing.is_empty(),
        "docs/ARCHITECTURE.md is missing {} of {} AppUiCommand variants.\n\
         Add a row for each under the matching `### <prefix>/` heading:\n  {}",
        missing.len(),
        variants.len(),
        missing
            .iter()
            .map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

#[test]
fn architecture_documents_every_source_file() {
    fn walk(dir: &Path, out: &mut Vec<String>, root: &Path) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out, root);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path.strip_prefix(root).expect("path under repo root");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join("src"), &mut files, &root);
    files.sort();
    assert!(
        files.len() > 20,
        "found only {} source files — the walk is wrong",
        files.len()
    );

    let doc = read("docs/ARCHITECTURE.md");
    let missing: Vec<&String> = files
        .iter()
        .filter(|f| !doc.contains(&format!("`{f}`")))
        .collect();

    assert!(
        missing.is_empty(),
        "docs/ARCHITECTURE.md's Client Layers table is missing {} of {} files \
         under src/.\nAdd a row for each:\n  {}",
        missing.len(),
        files.len(),
        missing
            .iter()
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

#[test]
fn architecture_documents_every_handled_notification() {
    // Every `UiNotification::Foo` the store matches on. The match is
    // exhaustive, so this is also the full set the pinned octos-core defines.
    let store = read("src/store.rs");
    let mut handled = BTreeSet::new();
    let mut rest = store.as_str();
    while let Some(idx) = rest.find("UiNotification::") {
        rest = &rest[idx + "UiNotification::".len()..];
        let ident: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if ident.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            handled.insert(ident);
        }
    }
    assert!(
        handled.len() > 30,
        "found only {} notification variants — the parser is wrong",
        handled.len()
    );

    let doc = read("docs/ARCHITECTURE.md");
    let missing: Vec<&String> = handled
        .iter()
        .filter(|v| !doc.contains(&format!("`{v}`")))
        .collect();

    assert!(
        missing.is_empty(),
        "docs/ARCHITECTURE.md is missing {} of {} UiNotification variants \
         the store handles:\n  {}",
        missing.len(),
        handled.len(),
        missing
            .iter()
            .map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// The counterweight: a table that lists files which no longer exist is just
/// as misleading as one that omits new ones.
#[test]
fn architecture_lists_no_deleted_source_files() {
    let doc = read("docs/ARCHITECTURE.md");
    let root = repo_root();
    let stale: Vec<String> = doc
        .lines()
        .filter(|line| line.trim_start().starts_with("| `src/"))
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("| `")?;
            let path = rest.split('`').next()?;
            (!root.join(path).exists()).then(|| path.to_string())
        })
        .collect();

    assert!(
        stale.is_empty(),
        "docs/ARCHITECTURE.md's Client Layers table lists {} path(s) that no \
         longer exist:\n  {}",
        stale.len(),
        stale.join("\n  ")
    );
}
