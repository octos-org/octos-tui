use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{Frame, layout::Rect};

use super::{
    KeyBinding, MenuFrame, MenuItem, MenuMode, MenuPreview as SpecPreview, MenuSpec,
    MenuStatusSpec,
    multi_select_view::{MultiSelectItem, MultiSelectPreview, MultiSelectView},
    selection_view::{SelectionItem, SelectionPreview, SelectionView},
};
use crate::theme::Palette;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuSurface {
    pub stack_path: Vec<String>,
    pub view: MenuView,
}

impl MenuSurface {
    pub(crate) fn selection(view: SelectionView) -> Self {
        Self {
            stack_path: Vec::new(),
            view: MenuView::Selection(view),
        }
    }

    pub(crate) fn multi_select(view: MultiSelectView) -> Self {
        Self {
            stack_path: Vec::new(),
            view: MenuView::MultiSelect(view),
        }
    }

    pub(crate) fn from_spec(
        spec: &MenuSpec,
        frame: Option<&MenuFrame>,
        stack_path: Vec<String>,
    ) -> Self {
        let selected = frame.map(|frame| frame.selected_index).unwrap_or(0);
        let search_query = frame
            .map(|frame| frame.search_query.clone())
            .filter(|query| !query.is_empty());
        let view = match &spec.mode {
            MenuMode::MultiSelect { allow_reorder, .. } => MenuView::MultiSelect(
                multi_select_view_from_spec(spec, selected, search_query, *allow_reorder),
            ),
            MenuMode::SingleSelect | MenuMode::Loading | MenuMode::Message => {
                MenuView::Selection(selection_view_from_spec(spec, selected, search_query))
            }
        };
        Self { stack_path, view }
    }

    pub(crate) fn from_status(status: &MenuStatusSpec, stack_path: Vec<String>) -> Self {
        let mut view = SelectionView::new(status.title.clone(), Vec::new());
        view.subtitle = Some(status.message.clone());
        view.footer_hint = status.footer_hint.clone();
        Self {
            stack_path,
            view: MenuView::Selection(view),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuView {
    Selection(SelectionView),
    MultiSelect(MultiSelectView),
}

pub(crate) fn height_hint(menu: &MenuSurface, terminal_width: u16) -> u16 {
    match &menu.view {
        MenuView::Selection(view) => view.height_hint(terminal_width),
        MenuView::MultiSelect(view) => view.height_hint(terminal_width),
    }
}

pub(crate) fn render_menu_surface(
    frame: &mut Frame<'_>,
    area: Rect,
    menu: &MenuSurface,
    palette: Palette,
) {
    match decorated_view(menu) {
        MenuView::Selection(view) => frame.render_widget(view.widget(palette), area),
        MenuView::MultiSelect(view) => frame.render_widget(view.widget(palette), area),
    }
}

fn decorated_view(menu: &MenuSurface) -> MenuView {
    let path = stack_label(&menu.stack_path);
    match &menu.view {
        MenuView::Selection(view) => {
            let mut view = view.clone();
            merge_path_into_subtitle(&mut view.subtitle, path.as_deref());
            MenuView::Selection(view)
        }
        MenuView::MultiSelect(view) => {
            let mut view = view.clone();
            merge_path_into_subtitle(&mut view.subtitle, path.as_deref());
            MenuView::MultiSelect(view)
        }
    }
}

fn stack_label(path: &[String]) -> Option<String> {
    if path.len() <= 1 {
        None
    } else {
        Some(path.join(" / "))
    }
}

fn merge_path_into_subtitle(subtitle: &mut Option<String>, path: Option<&str>) {
    let Some(path) = path else {
        return;
    };
    match subtitle {
        Some(subtitle) if !subtitle.is_empty() => {
            *subtitle = format!("{path} | {subtitle}");
        }
        _ => {
            *subtitle = Some(path.to_string());
        }
    }
}

fn selection_view_from_spec(
    spec: &MenuSpec,
    selected: usize,
    search_query: Option<String>,
) -> SelectionView {
    let mut view = SelectionView::new(
        spec.title.clone(),
        spec.items.iter().map(selection_item_from_spec).collect(),
    );
    view.subtitle = spec.subtitle.clone();
    view.search_query = search_query;
    view.search_placeholder = spec.searchable.then(|| {
        spec.search_placeholder
            .clone()
            .unwrap_or_else(|| "Filter options".into())
    });
    view.footer_hint = spec.footer_hint.clone();
    view.preview = spec.preview.as_ref().map(selection_preview_from_spec);
    view.selected = selected;
    view
}

fn selection_item_from_spec(item: &MenuItem) -> SelectionItem {
    SelectionItem {
        id: item.id.clone(),
        label: item.label.clone(),
        description: item.description.clone(),
        shortcut: item.shortcut.as_ref().map(key_binding_label),
        disabled_reason: item.disabled_reason.clone(),
        current: item.state.current,
        default: item.state.default,
        toggle: item.state.checked,
        loading: item.state.loading,
        required_valid: item.state.required_valid,
    }
}

fn multi_select_view_from_spec(
    spec: &MenuSpec,
    selected: usize,
    search_query: Option<String>,
    allow_reorder: bool,
) -> MultiSelectView {
    let mut checked_order = 0;
    let items = spec
        .items
        .iter()
        .map(|item| {
            let checked = item.state.checked.unwrap_or(false);
            let order = (allow_reorder && checked).then(|| {
                let order = checked_order;
                checked_order += 1;
                order
            });
            multi_select_item_from_spec(item, checked, order)
        })
        .collect();

    let mut view = MultiSelectView::new(spec.title.clone(), items);
    view.subtitle = spec.subtitle.clone();
    view.search_query = search_query;
    view.search_placeholder = spec.searchable.then(|| {
        spec.search_placeholder
            .clone()
            .unwrap_or_else(|| "Filter options".into())
    });
    view.footer_hint = spec.footer_hint.clone();
    view.preview = spec.preview.as_ref().map(multi_select_preview_from_spec);
    view.selected = selected;
    view.reorder_enabled = allow_reorder;
    view
}

fn multi_select_item_from_spec(
    item: &MenuItem,
    checked: bool,
    order: Option<usize>,
) -> MultiSelectItem {
    MultiSelectItem {
        id: item.id.clone(),
        label: item.label.clone(),
        description: item.description.clone(),
        shortcut: item.shortcut.as_ref().map(key_binding_label),
        disabled_reason: item.disabled_reason.clone(),
        checked,
        current: item.state.current,
        default: item.state.default,
        loading: item.state.loading,
        order,
    }
}

fn selection_preview_from_spec(preview: &SpecPreview) -> SelectionPreview {
    let (title, lines) = preview_lines(preview);
    SelectionPreview { title, lines }
}

fn multi_select_preview_from_spec(preview: &SpecPreview) -> MultiSelectPreview {
    let (title, lines) = preview_lines(preview);
    MultiSelectPreview { title, lines }
}

fn preview_lines(preview: &SpecPreview) -> (String, Vec<String>) {
    match preview {
        SpecPreview::Text { title, body } => (
            title.clone().unwrap_or_else(|| "Preview".into()),
            body.lines().map(str::to_string).collect(),
        ),
        SpecPreview::KeyValues { title, rows } => (
            title.clone().unwrap_or_else(|| "Preview".into()),
            rows.iter()
                .map(|row| format!("{}: {}", row.label, row.value))
                .collect(),
        ),
    }
}

fn key_binding_label(binding: &KeyBinding) -> String {
    let mut parts = Vec::new();
    if binding.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if binding.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if binding.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    parts.push(key_code_label(&binding.code));
    parts.join("+")
}

fn key_code_label(code: &KeyCode) -> String {
    match code {
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PgUp".into(),
        KeyCode::PageDown => "PgDn".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "Shift+Tab".into(),
        KeyCode::Delete => "Del".into(),
        KeyCode::Insert => "Ins".into(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Char(ch) => ch.to_string(),
        KeyCode::Null => "Null".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::CapsLock => "CapsLock".into(),
        KeyCode::ScrollLock => "ScrollLock".into(),
        KeyCode::NumLock => "NumLock".into(),
        KeyCode::PrintScreen => "PrintScreen".into(),
        KeyCode::Pause => "Pause".into(),
        KeyCode::Menu => "Menu".into(),
        KeyCode::KeypadBegin => "KeypadBegin".into(),
        KeyCode::Media(media) => format!("{media:?}"),
        KeyCode::Modifier(modifier) => format!("{modifier:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::ThemeName,
        menu::{
            MenuAction, MenuItemState, MenuPreviewRow,
            multi_select_view::{MultiSelectItem, MultiSelectView},
            selection_view::{SelectionItem, SelectionPreview, SelectionView},
        },
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn render_text(menu: &MenuSurface, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                render_menu_surface(
                    frame,
                    frame.area(),
                    menu,
                    Palette::for_theme(ThemeName::Slate),
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
    fn height_hint_uses_active_view() {
        let mut single = SelectionView::new(
            "Theme",
            vec![
                SelectionItem::new("one", "One"),
                SelectionItem::new("two", "Two"),
            ],
        );
        single.preview = Some(SelectionPreview::new("Preview", vec!["line".into()]));
        let menu = MenuSurface::selection(single);

        assert!(height_hint(&menu, 80) > height_hint(&menu, 140));
    }

    #[test]
    fn stack_path_is_rendered_with_active_menu() {
        let view = SelectionView::new("Child", vec![SelectionItem::new("one", "One")]);
        let mut menu = MenuSurface::selection(view);
        menu.stack_path = vec!["Root".into(), "Child".into()];

        let text = render_text(&menu, 80, 10);

        assert!(text.contains("Root / Child"));
        assert!(text.contains("One"));
    }

    #[test]
    fn multi_select_surface_renders() {
        let mut item = MultiSelectItem::new("state", "State");
        item.checked = true;
        let menu = MenuSurface::multi_select(MultiSelectView::new("Status", vec![item]));

        let text = render_text(&menu, 80, 10);

        assert!(text.contains("[x] State"));
    }

    #[test]
    fn menu_spec_maps_to_multi_select_surface() {
        let mut first = MenuItem::new("state", "State", MenuAction::Noop)
            .with_state(MenuItemState::checked(true))
            .with_shortcut(KeyBinding::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        first.description = Some("runtime state".into());
        let second = MenuItem::new("cwd", "Working directory", MenuAction::Noop)
            .with_state(MenuItemState::checked(false))
            .disabled("not available");
        let mut spec = MenuSpec::new(
            "statusline",
            "Status Line",
            MenuMode::MultiSelect {
                allow_reorder: true,
                min_selected: 0,
                max_selected: None,
            },
        )
        .with_items(vec![first, second])
        .searchable("Filter fields");
        spec.preview = Some(SpecPreview::KeyValues {
            title: Some("Preview".into()),
            rows: vec![MenuPreviewRow {
                label: "status".into(),
                value: "idle".into(),
            }],
        });
        let mut frame = MenuFrame::new("statusline");
        frame.selected_index = 1;
        frame.search_query = "work".into();

        let menu = MenuSurface::from_spec(
            &spec,
            Some(&frame),
            vec!["Settings".into(), "Status Line".into()],
        );
        let text = render_text(&menu, 120, 12);

        assert!(text.contains("Settings / Status Line"));
        assert!(text.contains("Search work"));
        assert!(text.contains("[x] 01 Ctrl+s State - runtime state"));
        assert!(text.contains("> [ ] -- Working directory"));
        assert!(text.contains("not available"));
        assert!(text.contains("status: idle"));
    }

    #[test]
    fn single_select_loading_state_is_rendered() {
        let spec = MenuSpec::new("provider", "Provider", MenuMode::SingleSelect).with_items(vec![
            MenuItem::new("provider.test", "Testing connection...", MenuAction::Noop).with_state(
                MenuItemState {
                    loading: true,
                    ..MenuItemState::default()
                },
            ),
        ]);
        let menu = MenuSurface::from_spec(&spec, None, vec!["Provider".into()]);
        let text = render_text(&menu, 80, 8);

        assert!(text.contains("[..] Testing connection..."));
    }
}
