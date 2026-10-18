//! Terminal delivery with a caller-owned durable journal. An opaque paste can
//! only be submitted when this delivery created it in an empty composer.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::delivery::{READ_LINES, input_is_collapsed, input_region};
use crate::{Delivery, DeliveryVia, Herdr};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum TextDeliveryState {
    Pasting,
    Pasted { marker: Option<String> },
    Entering { marker: Option<String> },
    Submitted,
}

impl Herdr {
    /// Persist intent before each terminal write. A resumed delivery never pastes
    /// again, even if a subprocess acknowledgement or the caller was lost.
    pub fn pane_send_text_recorded(
        &self,
        pane: &str,
        text: &str,
        state: Option<TextDeliveryState>,
        mut record: impl FnMut(&TextDeliveryState) -> Result<()>,
    ) -> Delivery {
        let mut delivery = Delivery {
            via: DeliveryVia::SendText,
            submitted: false,
            attempts: 0,
            error: None,
        };
        if let Err(error) = self.recorded_text(pane, text, state, &mut record, &mut delivery) {
            delivery.error = Some(format!("{error:#}"));
        }
        delivery
    }

    fn recorded_text(
        &self,
        pane: &str,
        text: &str,
        state: Option<TextDeliveryState>,
        record: &mut impl FnMut(&TextDeliveryState) -> Result<()>,
        delivery: &mut Delivery,
    ) -> Result<()> {
        ensure!(!text.trim().is_empty(), "cannot deliver empty pane input");
        if state == Some(TextDeliveryState::Submitted) {
            delivery.submitted = true;
            return Ok(());
        }
        let screen = self.pane_read(pane, READ_LINES)?;
        let (marker, entering) = match state {
            None => {
                ensure!(
                    !input_is_collapsed(&screen),
                    "cannot verify pre-existing collapsed paste; use --mark-delivered only after verifying delivery"
                );
                ensure!(
                    composer_empty(&screen),
                    "pane composer is not empty; refusing to append a reply"
                );
                record(&TextDeliveryState::Pasting)?;
                self.call(&["pane", "send-text", pane, text])?;
                (self.await_paste(pane, text)?, false)
            }
            Some(TextDeliveryState::Pasting) => {
                // The paste may have reached the terminal despite a lost ACK.
                // Empty or different input is ambiguous; never paste a second time.
                let marker = owned_marker(&screen, text);
                ensure!(
                    marker.is_some() || matches_input(&screen, text, None),
                    "paste outcome is uncertain; verify delivery before --mark-delivered"
                );
                (marker, false)
            }
            Some(TextDeliveryState::Pasted { marker }) => (marker, false),
            Some(TextDeliveryState::Entering { marker }) => (marker, true),
            Some(TextDeliveryState::Submitted) => unreachable!(),
        };
        let screen = self.pane_read(pane, READ_LINES)?;
        if !matches_input(&screen, text, marker.as_deref()) {
            ensure!(
                entering && composer_empty(&screen),
                "reply composer changed; verify delivery before --mark-delivered"
            );
            record(&TextDeliveryState::Submitted)?;
            delivery.submitted = true;
            return Ok(());
        }
        record(&TextDeliveryState::Pasted {
            marker: marker.clone(),
        })?;
        for attempt in 1..=crate::ENTER_ATTEMPTS {
            // Re-read before every Enter. A late acceptance must not submit the
            // next composer, and an unrelated draft must never be sent.
            let screen = self.pane_read(pane, READ_LINES)?;
            if !matches_input(&screen, text, marker.as_deref()) {
                ensure!(
                    composer_empty(&screen),
                    "reply composer changed after Enter"
                );
                ensure!(
                    attempt > 1 || entering,
                    "reply disappeared before submission"
                );
                record(&TextDeliveryState::Submitted)?;
                delivery.submitted = true;
                return Ok(());
            }
            record(&TextDeliveryState::Entering {
                marker: marker.clone(),
            })?;
            delivery.attempts = attempt;
            self.call(&["pane", "send-keys", pane, "enter"])?;
            for poll in 0..10 {
                if poll > 0 {
                    (self.poll_wait)(std::time::Duration::from_millis(100));
                }
                let screen = self.pane_read(pane, READ_LINES)?;
                if !matches_input(&screen, text, marker.as_deref()) {
                    ensure!(
                        composer_empty(&screen),
                        "reply composer changed after Enter"
                    );
                    record(&TextDeliveryState::Submitted)?;
                    delivery.submitted = true;
                    return Ok(());
                }
            }
        }
        anyhow::bail!(
            "pane input still contains reply after {} Enter attempts",
            crate::ENTER_ATTEMPTS
        )
    }

    fn await_paste(&self, pane: &str, text: &str) -> Result<Option<String>> {
        for poll in 0..10 {
            if poll > 0 {
                (self.poll_wait)(std::time::Duration::from_millis(100));
            }
            let screen = self.pane_read(pane, READ_LINES)?;
            if let Some(marker) = owned_marker(&screen, text) {
                return Ok(Some(marker));
            }
            if matches_input(&screen, text, None) {
                return Ok(None);
            }
        }
        anyhow::bail!("pasted text never appeared in pane input")
    }
}

fn composer_empty(screen: &str) -> bool {
    composer(screen).trim().is_empty()
}

fn composer(screen: &str) -> &str {
    let region = input_region(screen);
    let mut offset = 0;
    for line in region.split_inclusive('\n') {
        // OmO closes its multiline editor with a horizontal rule. Status and
        // help text below that rule are not part of the editable draft.
        let trimmed = line.trim();
        if trimmed.chars().count() >= 3
            && trimmed
                .chars()
                .all(|c| matches!(c, '\u{2500}' | '\u{2501}'))
        {
            return &region[..offset];
        }
        offset += line.len();
    }
    region
}

fn matches_input(screen: &str, text: &str, marker: Option<&str>) -> bool {
    match marker {
        Some(marker) => composer(screen).trim() == marker,
        None => {
            !input_is_collapsed(screen)
                && composer(screen)
                    .split_whitespace()
                    .eq(text.split_whitespace())
        }
    }
}

fn owned_marker(screen: &str, text: &str) -> Option<String> {
    let line = composer(screen).trim();
    let rest = line.strip_prefix("[paste #")?;
    let (number, size) = rest.split_once(' ')?;
    if number.parse::<u32>().is_err() {
        return None;
    }
    // OmO's JavaScript editor reports UTF-16 code units, not Unicode scalars.
    let chars = text.encode_utf16().count();
    let lines = text.split('\n').count();
    (size == format!("{chars} chars]") || size == format!("+{lines} lines]"))
        .then(|| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_input_requires_one_complete_marker_and_uses_editor_character_units() {
        // Given a real OmO editor boundary and a non-BMP character in the payload.
        let payload = "\u{1f600}".repeat(600);
        let screen = "> [paste #1 1200 chars]\n\u{2500}\u{2500}\u{2500}\nmodel status";
        // When inspecting a single marker versus additional hidden draft content.
        assert_eq!(
            owned_marker(screen, &payload).as_deref(),
            Some("[paste #1 1200 chars]")
        );
        // Then footer text is excluded, but no extra editable line can be ignored.
        assert!(owned_marker("> [paste #1 1200 chars]\nother draft", &payload).is_none());
        assert!(!composer_empty(
            "> \nother draft\n\u{2500}\u{2500}\u{2500}\nstatus"
        ));
        assert!(!matches_input("> prefix hello\n", "hello", None));
    }
}
