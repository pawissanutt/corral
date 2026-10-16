//! Colors and small widget helpers matching docs/mockup/corral-tui.html.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders};

pub const BG: Color = Color::Rgb(0x0c, 0x0f, 0x15);
pub const BG2: Color = Color::Rgb(0x13, 0x19, 0x26);
pub const BORDER: Color = Color::Rgb(0x26, 0x30, 0x42);
pub const FG: Color = Color::Rgb(0xc8, 0xd0, 0xdc);
pub const DIM: Color = Color::Rgb(0x6c, 0x76, 0x8a);
pub const CYAN: Color = Color::Rgb(0x5c, 0xcf, 0xe6);
pub const GREEN: Color = Color::Rgb(0x95, 0xd3, 0x6a);
pub const YELLOW: Color = Color::Rgb(0xe6, 0xc3, 0x5c);
pub const RED: Color = Color::Rgb(0xf0, 0x71, 0x78);
pub const MAGENTA: Color = Color::Rgb(0xc7, 0x92, 0xea);
pub const ORANGE: Color = Color::Rgb(0xff, 0xa7, 0x59);
pub const BLUE: Color = Color::Rgb(0x73, 0xa6, 0xff);
pub const SELECTED_BG: Color = Color::Rgb(0x12, 0x1c, 0x2a);

const PALETTE: [Color; 6] = [MAGENTA, CYAN, GREEN, ORANGE, BLUE, YELLOW];

/// Recolor cyan foreground cells after composition; other palette colors are untouched.
pub(crate) fn apply_accent(buffer: &mut ratatui::buffer::Buffer, accent: corral_core::Accent) {
    let color = match accent {
        corral_core::Accent::Cyan => return,
        corral_core::Accent::Blue => BLUE,
        corral_core::Accent::Magenta => MAGENTA,
        corral_core::Accent::Green => GREEN,
        corral_core::Accent::Orange => ORANGE,
    };
    for cell in &mut buffer.content {
        if cell.fg == CYAN {
            cell.set_fg(color);
        }
    }
}

/// Stable color for a project name (hash of the name into the palette).
pub fn project_color(name: &str) -> Color {
    let h = name
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    PALETTE[(h as usize) % PALETTE.len()]
}

/// Color for an agent key.
pub fn agent_color(agent: &str) -> Color {
    match agent {
        "omo" => BLUE,
        "omp" | "pi" => YELLOW,
        "claude" => ORANGE,
        _ => DIM,
    }
}

pub fn dim() -> Style {
    Style::new().fg(DIM)
}

pub fn bold() -> Style {
    Style::new().fg(FG).add_modifier(Modifier::BOLD)
}

/// Rounded-corner block with a bold title on the border, like the mock's boxes.
pub fn titled(title: impl Into<String>) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(Span::styled(format!(" {} ", title.into()), bold()))
}

/// Same as `titled` with the focused (cyan) title color.
pub fn titled_focused(title: impl Into<String>) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(Span::styled(
            format!(" {} ", title.into()),
            Style::new().fg(CYAN).add_modifier(Modifier::BOLD),
        ))
}
