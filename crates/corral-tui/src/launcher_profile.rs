//! The launcher's profile line: which agent profile the session it opens runs. It reads `default`
//! (each mode keeps its own configured profile) until the owner picks one with `Tab`/`Shift+Tab` or a
//! click; a picked profile names its exact model and how it is paid for, and one that cannot open a
//! session now says why, and no row opens while it is chosen.

use ratatui::text::Line;

use super::block::{LABEL_COL, Piece, labelled};
use super::{ACCENT, LauncherContext, MUTED, Part, State, TEXT, WARN, fg};

const PROFILE_LABEL: &str = "Profile";
/// The profile line's value before the owner picks one.
const DEFAULT_NAME: &str = "default";
const DEFAULT_NOTE: &str = " each mode keeps its own profile";

/// How a profile's model requests are paid for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Billing {
    /// A provider subscription login: `subscription`, muted.
    Subscription,
    /// An API key billed per request: `API · metered`, in warn.
    Metered,
    /// A model on this machine: `local`, muted.
    Local,
}

/// A profile the launcher offers for the session it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherProfile {
    /// The profile's exact config name; [`super::pick`] returns it when chosen.
    pub name: String,
    /// The exact model it runs, `provider/model`.
    pub model: String,
    pub billing: Billing,
    /// Why it cannot open a session now; `None` when it can. A profile with a reason is offered
    /// with it, and no row opens while it is chosen.
    pub unavailable: Option<String>,
}

/// `text` with control characters as spaces, so a config value cannot move the cursor.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The profile line, in the option block's label column: `Profile ‹ default ›` with its note, or
/// `Profile ‹ name ›` with the model, `subscription`, `API · metered` or `local`, and
/// `unavailable: <reason>` in warn. It is live whatever row is selected: the line moves to the next
/// profile on a click and to the previous one on `‹`. Without profiles there is no line.
pub(super) fn profile_line(
    state: &State,
    context: &LauncherContext,
    width: usize,
) -> Vec<(Line<'static>, Part)> {
    if context.profiles.is_empty() {
        return Vec::new();
    }
    let chevron = fg(ACCENT);
    let mut value: Vec<Piece> = vec![("‹".to_string(), chevron)];
    match state.profile().and_then(|i| context.profiles.get(i)) {
        None => {
            value.push((format!(" {DEFAULT_NAME} "), fg(TEXT)));
            value.push(("›".to_string(), chevron));
            value.push((DEFAULT_NOTE.to_string(), fg(MUTED)));
        }
        Some(profile) => {
            value.push((format!(" {} ", plain(&profile.name)), fg(TEXT)));
            value.push(("›".to_string(), chevron));
            value.push((format!(" {}", plain(&profile.model)), fg(MUTED)));
            let (word, tone) = match profile.billing {
                Billing::Subscription => ("subscription", MUTED),
                Billing::Metered => ("API · metered", WARN),
                Billing::Local => ("local", MUTED),
            };
            value.push((format!(" · {word}"), fg(tone)));
            if let Some(reason) = &profile.unavailable {
                value.push((format!(" · unavailable: {}", plain(reason)), fg(WARN)));
            }
        }
    }
    let mut out = Vec::new();
    labelled(
        &mut out,
        PROFILE_LABEL,
        &value,
        width.saturating_sub(LABEL_COL),
        |n| Part::Profile((n == 0).then_some(LABEL_COL)),
    );
    out
}
