//! Minimal GitHub Releases client for `update --check` and `doctor`.
//!
//! A plain blocking `reqwest` GET against the public Releases API — no auth is
//! required for public repos, but `OCTOS_TUI_GITHUB_TOKEN` is honored to dodge
//! the unauthenticated rate limit (design §A.2 / Risks).

use std::time::Duration;

use eyre::{Result, WrapErr, eyre};
use serde::Deserialize;

/// `owner/name` slug for the released TUI binary.
pub const GITHUB_REPO: &str = "octos-org/octos-tui";

const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/octos-org/octos-tui/releases/latest";
const RELEASES_URL: &str = "https://api.github.com/repos/octos-org/octos-tui/releases";
const API_BASE: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("octos-tui/", env!("CARGO_PKG_VERSION"));
const TIMEOUT: Duration = Duration::from_secs(10);

/// The release info `update`/`doctor` care about.
#[derive(Debug, Clone)]
pub struct LatestRelease {
    /// The release tag, e.g. `v0.1.2`.
    pub tag: String,
    /// Whether GitHub marked this release as a prerelease.
    pub prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct ReleasePayload {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(TIMEOUT)
        .build()
        .wrap_err("failed to build HTTP client")
}

fn token() -> Option<String> {
    std::env::var("OCTOS_TUI_GITHUB_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
}

fn authed(req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
    let req = req
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    match token() {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

/// Query the latest release. When `allow_prerelease` is set and the newest
/// release overall is a prerelease, prefer it; otherwise return the latest
/// stable (the dedicated `/releases/latest` endpoint, which excludes
/// prereleases and drafts).
pub fn latest_release(allow_prerelease: bool) -> Result<LatestRelease> {
    let client = client()?;

    if allow_prerelease {
        if let Some(release) = newest_including_prerelease(&client)? {
            return Ok(release);
        }
    }

    let resp = authed(client.get(RELEASES_LATEST_URL))
        .send()
        .wrap_err("failed to reach api.github.com")?;
    let status = resp.status();
    if !status.is_success() {
        return Err(eyre!(
            "GitHub returned {status} for the latest octos-tui release"
        ));
    }
    let payload: ReleasePayload = resp
        .json()
        .wrap_err("failed to decode GitHub release payload")?;
    Ok(LatestRelease {
        tag: payload.tag_name,
        prerelease: payload.prerelease,
    })
}

/// Newest non-draft release including prereleases (first entry of `/releases`,
/// which GitHub returns newest-first). Returns `None` if there are no releases.
fn newest_including_prerelease(
    client: &reqwest::blocking::Client,
) -> Result<Option<LatestRelease>> {
    let resp = authed(client.get(RELEASES_URL))
        .query(&[("per_page", "10")])
        .send()
        .wrap_err("failed to reach api.github.com")?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let payloads: Vec<ReleasePayload> = resp
        .json()
        .wrap_err("failed to decode GitHub releases list")?;
    Ok(payloads
        .into_iter()
        .find(|r| !r.draft)
        .map(|r| LatestRelease {
            tag: r.tag_name,
            prerelease: r.prerelease,
        }))
}

/// Whether `api.github.com` is reachable (a cheap GET against the API root).
/// Used by `doctor`'s network check; surfaces 403 rate-limit distinctly so the
/// caller can warn rather than fail.
pub fn reachability() -> Reachability {
    let client = match client() {
        Ok(c) => c,
        Err(_) => return Reachability::Unreachable("failed to build HTTP client".into()),
    };
    match authed(client.get(API_BASE)).send() {
        Ok(resp) if resp.status().is_success() => Reachability::Ok,
        Ok(resp) if resp.status().as_u16() == 403 => Reachability::RateLimited,
        Ok(resp) => Reachability::Unreachable(format!("HTTP {}", resp.status())),
        Err(err) => Reachability::Unreachable(err.to_string()),
    }
}

/// Result of the GitHub reachability probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    /// Reachable.
    Ok,
    /// Reachable but rate-limited (403) — warn, don't fail.
    RateLimited,
    /// Not reachable (network/proxy/DNS).
    Unreachable(String),
}
