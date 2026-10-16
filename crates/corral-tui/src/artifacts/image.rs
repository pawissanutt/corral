//! In-TUI image previews via ratatui-image. The graphics protocol (kitty, sixel, iTerm2) is detected once at
//! startup with [`init_picker`]; without detection, or when the terminal answers nothing usable (herdr), images
//! render as unicode halfblocks, which work in any truecolor terminal.

use std::sync::OnceLock;

use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

static PICKER: OnceLock<Picker> = OnceLock::new();

/// Queries the terminal for its graphics protocol and font size. Call once after the terminal entered raw
/// mode and before the event loop reads input; falls back to halfblocks when the query fails.
pub fn init_picker() {
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let _ = PICKER.set(picker);
}

fn picker() -> &'static Picker {
    PICKER.get_or_init(Picker::halfblocks)
}

/// Decodes `path` into a protocol state that resizes to whatever area it is drawn in.
pub(crate) fn load(path: &str) -> Result<StatefulProtocol> {
    let img = image::ImageReader::open(path)
        .with_context(|| format!("opening {path}"))?
        .with_guessed_format()
        .with_context(|| format!("reading {path}"))?
        .decode()
        .with_context(|| format!("decoding {path}"))?;
    Ok(picker().new_resize_protocol(img))
}

/// A gallery keeps many images resident; bound each decode and retain only a thumbnail.
pub(crate) fn thumbnail(path: &str) -> Result<StatefulProtocol> {
    let mut reader = image::ImageReader::open(path)
        .with_context(|| format!("opening {path}"))?
        .with_guessed_format()
        .with_context(|| format!("reading {path}"))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .with_context(|| format!("decoding thumbnail {path}"))?
        .thumbnail(600, 400);
    Ok(picker().new_resize_protocol(img))
}

/// Draws the image scaled (up or down, aspect kept) to fill `area`.
pub(crate) fn draw(f: &mut Frame, area: Rect, state: &mut StatefulProtocol) {
    let widget = StatefulImage::default().resize(Resize::Scale(None));
    f.render_stateful_widget(widget, area, state);
}
