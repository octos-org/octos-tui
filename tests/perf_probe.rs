//! One-off timing probe (ignored by default): how expensive is one pager
//! frame on a long transcript with many highlighted code blocks?
use octos_core::{Message, SessionKey};
use octos_tui::app;
use octos_tui::cli::ThemeName;
use octos_tui::model::{AppState, SessionView};
use octos_tui::theme::Palette;
use octos_tui::tui_terminal::FrameLike;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

struct BufferFrame {
    area: Rect,
    buffer: Buffer,
}
impl FrameLike for BufferFrame {
    fn area(&self) -> Rect {
        self.area
    }
    fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, &mut self.buffer);
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, _p: P) {}
    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }
}

#[test]
#[ignore]
fn time_pager_frame() {
    let code = "```rust\nfn main() {\n    let x: Vec<u32> = (0..100).map(|i| i * 2).collect();\n    println!(\"{:?}\", x);\n}\n```";
    let body = format!(
        "Here is some code:\n\n{code}\n\nAnd a paragraph explaining it in some detail with **bold** and `inline` bits."
    );
    let messages: Vec<Message> = (0..40)
        .flat_map(|i| {
            [
                Message::user(format!("question {i}")),
                Message::assistant(body.clone()),
            ]
        })
        .collect();
    let mut state = AppState::new(
        vec![SessionView {
            id: SessionKey("local:perf".into()),
            title: "perf".into(),
            profile_id: Some("coding".into()),
            messages,
            tasks: vec![],
            live_reply: None,
        }],
        0,
        "ready".into(),
        None,
        false,
    );
    state.transcript_pager_active = true;
    state.transcript_scroll = 50;
    let palette = Palette::for_theme(ThemeName::default());

    // warm up lazy syntax set
    let area = Rect::new(0, 0, 120, 40);
    let mut frame = BufferFrame {
        area,
        buffer: Buffer::empty(area),
    };
    app::render(&mut frame, &state, palette);

    let n = 20;
    let start = std::time::Instant::now();
    for i in 0..n {
        state.transcript_scroll = 50 + i; // simulate scrolling
        let mut frame = BufferFrame {
            area,
            buffer: Buffer::empty(area),
        };
        app::render(&mut frame, &state, palette);
    }
    let per_frame = start.elapsed() / n as u32;
    eprintln!("PER-FRAME: {per_frame:?}  (40 msgs with code blocks, 120x40)");
}
