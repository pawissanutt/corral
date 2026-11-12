//! Facts reported by the active daemon session, without fixture reach paths.

use crate::model::daemon::{self, Feed, Link};
use crate::model::nav::SettingsSection;

/// The daemon has no settings projection beyond connection facts and general guidance.
pub fn daemon_section_notice(section: SettingsSection) -> Option<String> {
    (!matches!(
        section,
        SettingsSection::Connection | SettingsSection::General
    ))
    .then(|| format!("corrald does not serve {} yet.", section.label()))
}

/// The connection inspector's label/value rows from the live session.
pub fn daemon_connection_details(feed: &Feed) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        (
            "Link",
            match feed.link {
                Link::Connecting => "connecting",
                Link::Up => "up",
                Link::Down => "down",
            }
            .to_owned(),
        ),
        (
            "Socket",
            feed.socket
                .clone()
                .unwrap_or_else(|| "not configured".into()),
        ),
        (
            "Store",
            feed.store.clone().unwrap_or_else(|| "not received".into()),
        ),
    ];
    if let Some(health) = &feed.health {
        rows.push(("Daemon version", health.version.clone()));
        rows.push((
            "Last pass",
            health.last_pass.map_or_else(
                || "never".into(),
                |at| {
                    let age = daemon::age(Some(at), feed.now);
                    match health.last_pass_ms {
                        Some(ms) => format!("{age} ago · took {:.1} s", ms as f64 / 1000.0),
                        None => format!("{age} ago"),
                    }
                },
            ),
        ));
    }
    if let Some(problem) = &feed.problem {
        rows.push(("Problem", problem.clone()));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_settings_hides_sections_without_real_data() {
        for section in SettingsSection::ALL {
            let notice = daemon_section_notice(section);
            if matches!(
                section,
                SettingsSection::Connection | SettingsSection::General
            ) {
                assert!(notice.is_none());
            } else {
                assert!(notice.is_some_and(|line| line.contains(section.label())));
            }
        }
    }
}
