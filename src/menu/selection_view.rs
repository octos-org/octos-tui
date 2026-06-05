use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::theme::Palette;

const WIDE_PREVIEW_WIDTH: u16 = 100;
const MAX_ITEMS: u16 = 8;
const NARROW_MAX_ITEMS: u16 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub shortcut: Option<String>,
    pub disabled_reason: Option<String>,
    pub current: bool,
    pub default: bool,
    pub toggle: Option<bool>,
    pub loading: bool,
    pub required_valid: Option<bool>,
}

impl SelectionItem {
    pub(crate) fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            shortcut: None,
            disabled_reason: None,
            current: false,
            default: false,
            toggle: None,
            loading: false,
            required_valid: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionPreview {
    pub title: String,
    pub lines: Vec<String>,
}

impl SelectionPreview {
    pub(crate) fn new(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            lines,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionView {
    pub title: String,
    pub subtitle: Option<String>,
    pub search_query: Option<String>,
    pub search_placeholder: Option<String>,
    pub footer_hint: Option<String>,
    pub items: Vec<SelectionItem>,
    pub selected: usize,
    pub scroll: usize,
    pub preview: Option<SelectionPreview>,
}

impl SelectionView {
    pub(crate) fn new(title: impl Into<String>, items: Vec<SelectionItem>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            search_query: None,
            search_placeholder: None,
            footer_hint: None,
            items,
            selected: 0,
            scroll: 0,
            preview: None,
        }
    }

    pub(crate) fn height_hint(&self, terminal_width: u16) -> u16 {
        let header_rows = u16::from(self.subtitle.is_some()) + u16::from(self.has_search_row());
        let max_items = if terminal_width >= WIDE_PREVIEW_WIDTH {
            MAX_ITEMS
        } else {
            NARROW_MAX_ITEMS
        };
        let item_rows = self.items.len().max(1).min(usize::from(max_items)) as u16;
        let stacked_preview_rows =
            u16::from(self.preview.is_some() && terminal_width < WIDE_PREVIEW_WIDTH) * 3;
        2 + header_rows + item_rows + stacked_preview_rows + 1
    }

    pub(crate) fn widget(&self, palette: Palette) -> SelectionViewWidget<'_> {
        SelectionViewWidget {
            view: self,
            palette,
        }
    }

    fn has_search_row(&self) -> bool {
        self.search_query.is_some() || self.search_placeholder.is_some()
    }

    fn selected_index(&self) -> usize {
        self.selected.min(self.items.len().saturating_sub(1))
    }
}

pub(crate) struct SelectionViewWidget<'a> {
    view: &'a SelectionView,
    palette: Palette,
}

impl Widget for SelectionViewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .style(
                Style::default()
                    .fg(self.palette.text)
                    .bg(self.palette.surface),
            )
            .border_style(self.palette.border())
            .title(Line::from(Span::styled(
                self.view.title.clone(),
                self.palette.title().add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        render_footer(
            chunks[1],
            buf,
            self.palette,
            self.view.footer_hint.as_deref(),
            "Enter accept | Esc cancel | Up/Down move",
        );

        if self.view.preview.is_some() && inner.width >= WIDE_PREVIEW_WIDTH {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                // 2-col gutter so a clipped list item can't butt straight into
                // the preview text (the "Aliasubmission" collision).
                .spacing(2)
                .split(chunks[0]);
            render_selection_list(self.view, body[0], buf, self.palette);
            render_preview(self.view.preview.as_ref(), body[1], buf, self.palette);
        } else if self.view.preview.is_some() && chunks[0].height >= 8 {
            let preview_height = chunks[0].height.min(4);
            let body = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(preview_height)])
                .split(chunks[0]);
            render_selection_list(self.view, body[0], buf, self.palette);
            render_preview(self.view.preview.as_ref(), body[1], buf, self.palette);
        } else {
            render_selection_list(self.view, chunks[0], buf, self.palette);
        }
    }
}

fn render_selection_list(view: &SelectionView, area: Rect, buf: &mut Buffer, palette: Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let header_height = u16::from(view.subtitle.is_some()) + u16::from(view.has_search_row());
    let chunks = if header_height == 0 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(0), Constraint::Min(1)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(header_height), Constraint::Min(1)])
            .split(area)
    };

    let mut header = Vec::new();
    if let Some(subtitle) = &view.subtitle {
        header.push(Line::from(Span::styled(subtitle.clone(), palette.muted())));
    }
    if view.has_search_row() {
        let query = view
            .search_query
            .as_deref()
            .filter(|query| !query.is_empty())
            .or(view.search_placeholder.as_deref())
            .unwrap_or_default();
        header.push(Line::from(vec![
            Span::styled("Search ", palette.title()),
            Span::styled(query.to_string(), palette.text()),
        ]));
    }
    Paragraph::new(Text::from(header))
        .style(Style::default().bg(palette.surface))
        .render(chunks[0], buf);

    let lines = selection_rows(
        view,
        chunks[1].height,
        usize::from(chunks[1].width),
        palette,
    );
    Paragraph::new(Text::from(lines))
        .style(Style::default().bg(palette.surface))
        .wrap(Wrap { trim: false })
        .render(chunks[1], buf);
}

fn selection_rows(
    view: &SelectionView,
    height: u16,
    width: usize,
    palette: Palette,
) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    if view.items.is_empty() {
        return vec![Line::from(Span::styled(
            "No options available",
            palette.muted(),
        ))];
    }

    let selected = view.selected_index();
    let start = visible_start(view.items.len(), selected, view.scroll, usize::from(height));
    let mut rows = Vec::new();
    for (idx, item) in view
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(usize::from(height))
    {
        rows.push(selection_row(item, idx == selected, width, palette));
    }
    rows
}

fn visible_start(total: usize, selected: usize, scroll: usize, height: usize) -> usize {
    if total == 0 || height == 0 {
        return 0;
    }
    let max_start = total.saturating_sub(height);
    let mut start = scroll.min(max_start);
    if selected < start {
        start = selected;
    } else if selected >= start + height {
        start = selected + 1 - height;
    }
    start.min(max_start)
}

fn selection_row(
    item: &SelectionItem,
    selected: bool,
    width: usize,
    palette: Palette,
) -> Line<'static> {
    let disabled = item.disabled_reason.is_some();
    let semantic = item.required_valid.map(|valid| {
        if valid {
            palette.success
        } else {
            palette.danger
        }
    });
    let base = if let Some(color) = semantic {
        let style = Style::default().fg(color);
        if selected {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    } else if disabled {
        palette.muted()
    } else if selected {
        palette.selected().add_modifier(Modifier::BOLD)
    } else {
        palette.text()
    };
    let style = if selected {
        base.bg(palette.surface_alt)
    } else {
        base.bg(palette.surface)
    };
    let muted = if selected {
        palette.muted().bg(palette.surface_alt)
    } else {
        palette.muted().bg(palette.surface)
    };
    let reason_style = if selected {
        Style::default().fg(palette.danger).bg(palette.surface_alt)
    } else {
        Style::default().fg(palette.danger).bg(palette.surface)
    };

    let marker = if selected { ">" } else { " " };
    let mut text = format!("{marker} ");
    if let Some(checked) = item.toggle {
        text.push_str(if checked { "[x] " } else { "[ ] " });
    }
    if let Some(shortcut) = &item.shortcut {
        text.push_str(shortcut);
        text.push(' ');
    }
    if item.loading {
        text.push_str("[..] ");
    }
    text.push_str(&item.label);
    if let Some(description) = &item.description {
        text.push_str(" - ");
        text.push_str(description);
    }
    if item.current {
        text.push_str(" current");
    }
    if item.default {
        text.push_str(" default");
    }

    let mut spans = vec![Span::styled(fit_text(&text, width), style)];
    if let Some(reason) = &item.disabled_reason {
        spans.push(Span::styled(
            fit_text(
                &format!(" ({reason})"),
                width.saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str())),
            ),
            reason_style,
        ));
    } else if item.id.is_empty() {
        spans.push(Span::styled("", muted));
    }
    Line::from(spans).style(if selected {
        Style::default().bg(palette.surface_alt)
    } else {
        Style::default().bg(palette.surface)
    })
}

fn render_preview(
    preview: Option<&SelectionPreview>,
    area: Rect,
    buf: &mut Buffer,
    palette: Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(preview) = preview else {
        return;
    };
    let mut lines = vec![Line::from(Span::styled(
        preview.title.clone(),
        palette.title().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(
        preview
            .lines
            .iter()
            .take(usize::from(area.height.saturating_sub(1)))
            .map(|line| Line::from(Span::styled(line.clone(), palette.text()))),
    );
    Paragraph::new(Text::from(lines))
        .style(Style::default().fg(palette.text).bg(palette.surface_alt))
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

fn render_footer(
    area: Rect,
    buf: &mut Buffer,
    palette: Palette,
    hint: Option<&str>,
    fallback: &str,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = hint.unwrap_or(fallback);
    Paragraph::new(Line::from(Span::styled(
        fit_text(text, usize::from(area.width)),
        palette.muted().bg(palette.surface),
    )))
    .style(Style::default().bg(palette.surface))
    .render(area, buf);
}

fn fit_text(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    // `width` is a COLUMN budget, not a char count: CJK glyphs occupy 2
    // display columns. Accumulate unicode display width so translated/CJK
    // labels truncate on a column boundary instead of overflowing the row.
    use unicode_width::UnicodeWidthChar;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ThemeName;
    use ratatui::{Terminal, backend::TestBackend};

    /// i18n/CJK: `fit_text`'s `width` is a column budget. CJK glyphs are
    /// double-width, so truncation must count display columns, not chars —
    /// otherwise translated menu labels overflow the row and misalign.
    #[test]
    fn fit_text_truncates_on_column_width_not_char_count() {
        assert_eq!(fit_text("hello", 3), "hel"); // ASCII: 1 col/char
        assert_eq!(fit_text("中文测试", 4), "中文"); // each CJK = 2 cols → 2 glyphs
        assert_eq!(fit_text("中文测试", 5), "中文"); // 3rd glyph would be col 6 > 5
        assert_eq!(fit_text("a中b", 3), "a中"); // 1 + 2 = 3 cols exactly
        assert_eq!(fit_text("中", 1), ""); // a 2-col glyph cannot fit in 1 col
    }

    fn render_buffer(view: &SelectionView, width: u16, height: u16, palette: Palette) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| frame.render_widget(view.widget(palette), frame.area()))
            .expect("render succeeds");
        terminal.backend().buffer().clone()
    }

    fn render_view(view: &SelectionView, width: u16, height: u16) -> String {
        render_buffer(view, width, height, Palette::for_theme(ThemeName::Slate))
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn style_for_text(buffer: &Buffer, needle: &str) -> Option<Style> {
        let width = usize::from(buffer.area.width);
        let height = usize::from(buffer.area.height);
        for y in 0..height {
            let row_start = y * width;
            let row = buffer.content[row_start..row_start + width]
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            if let Some(x) = row.find(needle) {
                let cell = &buffer.content[row_start + x];
                return Some(
                    Style::default()
                        .fg(cell.fg)
                        .bg(cell.bg)
                        .add_modifier(cell.modifier),
                );
            }
        }
        None
    }

    #[test]
    fn renders_selected_disabled_and_marked_rows() {
        let mut current = SelectionItem::new("current", "Current model");
        current.current = true;
        let mut disabled = SelectionItem::new("disabled", "Disabled model");
        disabled.disabled_reason = Some("server unavailable".into());
        let mut default = SelectionItem::new("default", "Default model");
        default.default = true;
        let mut view = SelectionView::new("Model", vec![current, disabled, default]);
        view.selected = 1;

        let text = render_view(&view, 80, 10);

        assert!(text.contains("> Disabled model"));
        assert!(text.contains("server unavailable"));
        assert!(text.contains("Current model current"));
        assert!(text.contains("Default model default"));
    }

    #[test]
    fn renders_required_rows_with_success_and_danger_colors() {
        let mut missing = SelectionItem::new("missing", "API key: not set");
        missing.required_valid = Some(false);
        let mut ready = SelectionItem::new("ready", "Model: deepseek-reasoner");
        ready.required_valid = Some(true);
        let view = SelectionView::new("Provider", vec![missing, ready]);
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = render_buffer(&view, 80, 8, palette);

        let missing_style = style_for_text(&buffer, "API key").expect("missing row style");
        let ready_style = style_for_text(&buffer, "Model").expect("ready row style");

        assert_eq!(missing_style.fg, Some(palette.danger));
        assert_eq!(ready_style.fg, Some(palette.success));
    }

    #[test]
    fn wide_layout_renders_side_preview() {
        let mut view = SelectionView::new(
            "Theme",
            vec![
                SelectionItem::new("slate", "Slate"),
                SelectionItem::new("terminal", "Terminal"),
            ],
        );
        view.preview = Some(SelectionPreview::new(
            "Preview",
            vec!["Surface".into(), "Accent".into()],
        ));

        let text = render_view(&view, 120, 12);

        assert!(text.contains("Preview"));
        assert!(text.contains("Surface"));
        assert!(text.contains("Slate"));
    }
}
