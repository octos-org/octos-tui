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
pub(crate) struct MultiSelectItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub shortcut: Option<String>,
    pub disabled_reason: Option<String>,
    pub checked: bool,
    pub current: bool,
    pub default: bool,
    pub loading: bool,
    pub order: Option<usize>,
}

impl MultiSelectItem {
    pub(crate) fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            shortcut: None,
            disabled_reason: None,
            checked: false,
            current: false,
            default: false,
            loading: false,
            order: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultiSelectPreview {
    pub title: String,
    pub lines: Vec<String>,
}

impl MultiSelectPreview {
    pub(crate) fn new(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            lines,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultiSelectView {
    pub title: String,
    pub subtitle: Option<String>,
    pub search_query: Option<String>,
    pub search_placeholder: Option<String>,
    pub footer_hint: Option<String>,
    pub items: Vec<MultiSelectItem>,
    pub selected: usize,
    pub scroll: usize,
    pub preview: Option<MultiSelectPreview>,
    pub reorder_enabled: bool,
}

impl MultiSelectView {
    pub(crate) fn new(title: impl Into<String>, items: Vec<MultiSelectItem>) -> Self {
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
            reorder_enabled: false,
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

    pub(crate) fn widget(&self, palette: Palette) -> MultiSelectViewWidget<'_> {
        MultiSelectViewWidget {
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

pub(crate) struct MultiSelectViewWidget<'a> {
    view: &'a MultiSelectView,
    palette: Palette,
}

impl Widget for MultiSelectViewWidget<'_> {
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

        render_footer(chunks[1], buf, self.palette, self.view);

        if self.view.preview.is_some() && inner.width >= WIDE_PREVIEW_WIDTH {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                // 2-col gutter so a clipped list item can't butt straight into
                // the preview text (the "Aliasubmission" collision).
                .spacing(2)
                .split(chunks[0]);
            render_item_list(self.view, body[0], buf, self.palette);
            render_preview(self.view.preview.as_ref(), body[1], buf, self.palette);
        } else if self.view.preview.is_some() && chunks[0].height >= 8 {
            let preview_height = chunks[0].height.min(4);
            let body = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(preview_height)])
                .split(chunks[0]);
            render_item_list(self.view, body[0], buf, self.palette);
            render_preview(self.view.preview.as_ref(), body[1], buf, self.palette);
        } else {
            render_item_list(self.view, chunks[0], buf, self.palette);
        }
    }
}

fn render_item_list(view: &MultiSelectView, area: Rect, buf: &mut Buffer, palette: Palette) {
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

    let lines = item_rows(
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

fn item_rows(
    view: &MultiSelectView,
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
        rows.push(item_row(
            item,
            idx == selected,
            view.reorder_enabled,
            width,
            palette,
        ));
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

fn item_row(
    item: &MultiSelectItem,
    selected: bool,
    reorder_enabled: bool,
    width: usize,
    palette: Palette,
) -> Line<'static> {
    let disabled = item.disabled_reason.is_some();
    let base = if disabled {
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
    let reason_style = if selected {
        Style::default().fg(palette.danger).bg(palette.surface_alt)
    } else {
        Style::default().fg(palette.danger).bg(palette.surface)
    };

    let marker = if selected { ">" } else { " " };
    let checkbox = if item.checked { "[x]" } else { "[ ]" };
    let order = if reorder_enabled {
        item.order
            .map(|order| format!("{:02} ", order + 1))
            .unwrap_or_else(|| "-- ".into())
    } else {
        String::new()
    };

    // `*` marks the active/current selection (clearer than a trailing "current").
    let current_marker = if item.current { "*" } else { " " };
    let mut text = format!("{marker}{current_marker} {checkbox} {order}");
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
        spans.push(Span::styled("", style));
    }
    Line::from(spans).style(if selected {
        Style::default().bg(palette.surface_alt)
    } else {
        Style::default().bg(palette.surface)
    })
}

fn render_preview(
    preview: Option<&MultiSelectPreview>,
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

fn render_footer(area: Rect, buf: &mut Buffer, palette: Palette, view: &MultiSelectView) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let fallback = if view.reorder_enabled {
        "Space toggle | Enter confirm | Esc cancel | Alt+Up/Alt+Down reorder"
    } else {
        "Space toggle | Enter confirm | Esc cancel | Up/Down move"
    };
    let text = view.footer_hint.as_deref().unwrap_or(fallback);
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

    fn render_view(view: &MultiSelectView, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                frame.render_widget(
                    view.widget(Palette::for_theme(ThemeName::Slate)),
                    frame.area(),
                )
            })
            .expect("render succeeds");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn renders_checkboxes_and_reorder_indices() {
        let mut first = MultiSelectItem::new("state", "State");
        first.checked = true;
        first.order = Some(0);
        let mut second = MultiSelectItem::new("cwd", "Working directory");
        second.order = Some(1);
        let mut view = MultiSelectView::new("Status Line", vec![first, second]);
        view.selected = 1;
        view.reorder_enabled = true;

        let text = render_view(&view, 90, 10);

        assert!(text.contains("[x] 01 State"));
        assert!(text.contains(">  [ ] 02 Working directory"));
        assert!(text.contains("Alt+Up/Alt+Down reorder"));
    }

    #[test]
    fn renders_disabled_reason_and_preview() {
        let mut item = MultiSelectItem::new("usage", "Usage");
        item.disabled_reason = Some("not in snapshot".into());
        let mut view = MultiSelectView::new("Title", vec![item]);
        view.preview = Some(MultiSelectPreview::new(
            "Preview",
            vec!["octos - project".into()],
        ));

        let text = render_view(&view, 120, 10);

        assert!(text.contains("not in snapshot"));
        assert!(text.contains("Preview"));
        assert!(text.contains("octos - project"));
    }
}
