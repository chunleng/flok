use std::sync::{Arc, RwLock};

use portable_pty::{MasterPty, PtySize};
use ratatui::{
    style::{Style, Stylize},
    text::Line,
    widgets::{Block, Paragraph, Widget},
};

pub struct AutoFillPty {
    pub pty: Arc<Box<dyn MasterPty + Send + 'static>>,
    pub parser: Arc<RwLock<vt100::Parser>>,
    pub title: String,
}

impl AutoFillPty {
    pub fn new(
        pty: Arc<Box<dyn MasterPty + Send + 'static>>,
        parser: Arc<RwLock<vt100::Parser>>,
        title: String,
    ) -> Self {
        Self { pty, parser, title }
    }
}

impl Widget for AutoFillPty {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        // Resize PTY and parser to match the layout (accounting for borders)
        let pty_cols = area.width.saturating_sub(2);
        let pty_rows = area.height.saturating_sub(2);
        self.pty
            .resize(PtySize {
                rows: pty_rows,
                cols: pty_cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();

        // Get the screen contents from the VT100 parser with colors
        self.parser.write().unwrap().set_size(pty_rows, pty_cols);
        let parser = self.parser.read().unwrap();
        let screen = parser.screen();

        let lines: Vec<Line> = (0..pty_rows)
            .map(|row| {
                let mut spans = vec![];
                let mut current_text = String::new();
                let mut current_style = Style::default();

                for col in 0..pty_cols {
                    let cell = screen.cell(row, col);
                    if let Some(cell) = cell {
                        let fg = cell.fgcolor();
                        let bg = cell.bgcolor();
                        let is_bold = cell.bold();
                        let is_italic = cell.italic();
                        let is_underline = cell.underline();

                        let mut style = Style::default();

                        // Convert VT100 colors to Ratatui colors
                        if let vt100::Color::Idx(idx) = fg {
                            style = style.fg(ansi_to_ratatui_color(idx));
                        }
                        if let vt100::Color::Idx(idx) = bg {
                            style = style.bg(ansi_to_ratatui_color(idx));
                        }
                        if is_bold {
                            style = style.bold();
                        }
                        if is_italic {
                            style = style.italic();
                        }
                        if is_underline {
                            style = style.underlined();
                        }

                        if style != current_style && !current_text.is_empty() {
                            spans.push(ratatui::text::Span::styled(
                                current_text.clone(),
                                current_style,
                            ));
                            current_text.clear();
                        }

                        current_style = style;
                        current_text.push_str(&cell.contents());
                    }
                }

                if !current_text.is_empty() {
                    spans.push(ratatui::text::Span::styled(current_text, current_style));
                }

                Line::from(spans)
            })
            .collect();
        Paragraph::new(lines)
            .block(Block::bordered().title(self.title))
            .render(area, buf);
    }
}

// Convert ANSI color index to Ratatui color
fn ansi_to_ratatui_color(idx: u8) -> ratatui::style::Color {
    use ratatui::style::Color;
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        _ => Color::Reset,
    }
}
