//! Feed-backed work content descriptors: the connection banner and a view's staleness.

use super::Button;
use crate::model::hue::Hue;
use crate::model::text::Rich;

/// A connection banner with its entire inline emphasis and handled controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    /// Connection state hue.
    pub hue: Hue,
    /// Complete message; the renderer inserts no additional wording.
    pub text: Rich,
    /// Desktop/terminal controls; the phone banner has none.
    pub actions: Vec<Button>,
}

/// Staleness flags and text used by every work-domain screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Dim and desaturate board content when the daemon is down or the device is offline.
    pub dimmed: bool,
    /// Snapshot timestamp to accompany stale views.
    pub as_of: Option<String>,
    /// Connection banner; healthy and relay connections have no banner.
    pub banner: Option<Banner>,
}
