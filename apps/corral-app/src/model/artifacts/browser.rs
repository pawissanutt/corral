//! Opening a standalone HTML artifact in the browser: the whole saved artifact is read again
//! from corrald page by page (as Download reads it) and must match the catalog's size and
//! SHA-256 before the platform's isolated preview launches; a partial head never does. The
//! preview is a sandboxed copy without the app's origin, so its scripts reach no credentials.

use super::Save;
use super::download::Saving;

pub const OPEN_IN_BROWSER: &str = "Open in browser";
pub const CANCEL_OPEN: &str = "Cancel open";
pub const VIEW_SOURCE: &str = "View source";
/// Said beside Open in browser: what the opened copy may do.
pub const SANDBOXED: &str = "Opens a sandboxed copy of the saved artifact. Its scripts can run and may navigate the frame to external sites; open only pages you trust.";
/// Said where this client cannot open a browser.
pub const UNSUPPORTED: &str =
    "This client cannot open a browser; View source shows the markup and Download saves the page.";

/// The largest page opened: the preview stages the whole page before it launches.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Why `save`'s artifact cannot open in a browser; none when it can.
pub fn refusal(save: &Save) -> Option<String> {
    (save.bytes > MAX_BYTES).then(|| {
        format!(
            "This page is larger than the {} a browser preview opens; Download saves it.",
            crate::model::session::transcript::size(MAX_BYTES)
        )
    })
}

/// The line under the actions while an open is under way or after it ended.
pub fn note(status: &Saving) -> Option<String> {
    match status {
        Saving::Idle => None,
        Saving::Choosing => Some("Preparing the preview…".into()),
        Saving::Reading { percent, .. } => Some(format!("Reading the page… {percent}%")),
        Saving::Finishing => Some("Launching browser…".into()),
        Saving::Saved(_) => Some("Opened in the browser.".into()),
        Saving::Failed(why) => Some(format!("Could not open in a browser: {why}")),
    }
}
