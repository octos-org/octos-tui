//! Fenced-code-block syntax highlighting for the transcript renderer
//! (`specs/task-code-syntax-highlight.spec`).
//!
//! syntect (pure-Rust `fancy-regex` engine) drives per-line token coloring.
//! Only FOREGROUND colors are taken from the theme: code blocks must keep
//! blending with the terminal's default background in the live tail, native
//! scrollback, and the pager (the repo-wide "no message backgrounds"
//! invariant). Unknown languages, missing fence tags, and oversized blocks
//! fall back to the caller's single-color style.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::OnceLock;

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Blocks longer than this fall back to single-color rendering: the live tail
/// re-renders every frame while a block is still streaming, and the frame
/// budget must not scale with pathological block sizes.
pub const HIGHLIGHT_MAX_LINES: usize = 300;

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

const FALLBACK_THEME: &str = "base16-eighties.dark";

fn theme_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

/// Resolve a syntect theme by name (the UI palette maps each `/theme` choice
/// to one); unknown names fall back rather than panic.
fn theme(name: &str) -> &'static Theme {
    let themes = &theme_set().themes;
    themes
        .get(name)
        .or_else(|| themes.get(FALLBACK_THEME))
        .or_else(|| themes.values().next())
        .expect("syntect default theme set is never empty")
}

/// Per-block highlighter: holds syntect's line-to-line state so multi-line
/// constructs (block comments, raw strings) color correctly. `None` inside
/// means "no recognized language" — the caller's fallback style applies.
pub struct CodeHighlighter {
    inner: Option<HighlightLines<'static>>,
}

impl CodeHighlighter {
    /// Resolve the fence's language token (`rust`, `py`, `json`, …). An empty
    /// or unrecognized token yields a pass-through highlighter — never a
    /// guess, never a panic.
    pub fn for_language(token: &str, theme_name: &str) -> Self {
        let token = token.trim();
        let syntax: Option<&'static SyntaxReference> = if token.is_empty() || token == "code" {
            None
        } else {
            syntax_set().find_syntax_by_token(token)
        };
        Self {
            inner: syntax.map(|syntax| HighlightLines::new(syntax, theme(theme_name))),
        }
    }

    /// Highlight one code line into spans, taking ONLY foreground colors from
    /// the theme. Falls back to `fallback` (single style for the whole line)
    /// when no language is active or highlighting errors out mid-stream.
    pub fn highlight_line(&mut self, line: &str, fallback: Style) -> Vec<Span<'static>> {
        let Some(highlighter) = self.inner.as_mut() else {
            return vec![Span::styled(line.to_string(), fallback)];
        };
        // The `newlines` syntax set expects line terminators; feed one and
        // strip it from the produced spans.
        let with_newline = format!("{line}\n");
        match highlighter.highlight_line(&with_newline, syntax_set()) {
            Ok(regions) => regions
                .into_iter()
                .filter_map(|(style, text)| {
                    let text = text.trim_end_matches('\n');
                    if text.is_empty() {
                        return None;
                    }
                    let fg = style.foreground;
                    Some(Span::styled(
                        text.to_string(),
                        Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b)),
                    ))
                })
                .collect(),
            Err(_) => vec![Span::styled(line.to_string(), fallback)],
        }
    }

    /// True when a recognized language is active (used by tests and the
    /// renderer's oversized-block fallback decision).
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }
}

/// Upper bound on memoized blocks; on overflow the cache is simply cleared
/// (correctness is unaffected — entries are pure functions of their key).
const BLOCK_CACHE_CAP: usize = 256;

thread_local! {
    static BLOCK_CACHE: RefCell<HashMap<u64, Rc<Vec<Vec<Span<'static>>>>>> =
        RefCell::new(HashMap::new());
}

/// Highlight a complete fenced block, memoized by `(language, body)`.
///
/// Committed transcript blocks are immutable, yet the pager re-renders every
/// one of them on each scroll frame — memoization turns the per-frame
/// highlight cost from O(all history) into O(still-streaming code). Set
/// `cacheable: false` for a block whose fence has not closed yet: its body
/// grows every frame and would only churn the cache.
pub fn highlight_block(
    language: &str,
    body: &[String],
    fallback: Style,
    cacheable: bool,
    theme_name: &str,
) -> Rc<Vec<Vec<Span<'static>>>> {
    let render = |language: &str, body: &[String]| -> Vec<Vec<Span<'static>>> {
        let mut highlighter = CodeHighlighter::for_language(language, theme_name);
        body.iter()
            .enumerate()
            .map(|(idx, line)| {
                if idx < HIGHLIGHT_MAX_LINES {
                    highlighter.highlight_line(line, fallback)
                } else {
                    vec![Span::styled(line.clone(), fallback)]
                }
            })
            .collect()
    };

    if !cacheable {
        return Rc::new(render(language, body));
    }

    let mut hasher = DefaultHasher::new();
    language.hash(&mut hasher);
    body.hash(&mut hasher);
    // The fallback fg AND theme name participate: /theme switches must not
    // serve stale colors from the previous theme.
    fallback.fg.hash(&mut hasher);
    theme_name.hash(&mut hasher);
    let key = hasher.finish();

    BLOCK_CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(&key) {
            return Rc::clone(hit);
        }
        let rendered = Rc::new(render(language, body));
        let mut cache = cache.borrow_mut();
        if cache.len() >= BLOCK_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, Rc::clone(&rendered));
        rendered
    })
}
