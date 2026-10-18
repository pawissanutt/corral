use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Herdr;

/// The transport that actually attempted delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryVia {
    AgentPrompt,
    SendText,
    OmoQuestion,
}

/// Submission evidence, including failures that callers must retain as pending work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub via: DeliveryVia,
    pub submitted: bool,
    /// Native prompt/question submission calls, or Enter attempts for send-text
    /// (not paste/read calls). Reconciliation without submission has zero attempts.
    pub attempts: u32,
    pub error: Option<String>,
}

pub(crate) const READ_LINES: usize = 200;
const POLLS: usize = 10;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Maximum Enter submissions, including the initial attempt, for terminal delivery.
pub const ENTER_ATTEMPTS: u32 = 3;

impl Herdr {
    /// Production and tests execute the same bounded poller. Only the wait is injected:
    /// production waits between reads; deterministic fakes advance on commands, not time.
    pub(crate) fn send_text_with_wait(
        &self,
        pane: &str,
        text: &str,
        submit: bool,
        wait: &mut impl FnMut(Duration),
    ) -> Delivery {
        let mut delivery = Delivery {
            via: DeliveryVia::SendText,
            submitted: false,
            attempts: 0,
            error: None,
        };
        let outcome = self.deliver_text(pane, text, submit, wait, &mut delivery);
        if let Err(error) = outcome {
            delivery.error = Some(format!("{error:#}"));
        }
        delivery
    }

    fn deliver_text(
        &self,
        pane: &str,
        text: &str,
        submit: bool,
        wait: &mut impl FnMut(Duration),
        delivery: &mut Delivery,
    ) -> Result<()> {
        anyhow::ensure!(
            !normalize(text).is_empty(),
            "cannot verify empty pane input"
        );
        // Read before any write: resending a pending reply must not paste it twice.
        let screen = self.pane_read(pane, READ_LINES)?;
        anyhow::ensure!(
            !input_is_collapsed(&screen),
            "cannot verify collapsed paste in pane input; refusing another paste"
        );
        if !input_contains(&screen, text) {
            self.call(&["pane", "send-text", pane, text])?;
            anyhow::ensure!(
                self.poll_input(pane, text, true, wait)?,
                "pasted text never appeared in pane input"
            );
        }
        if !submit {
            return Ok(());
        }
        for attempt in 1..=ENTER_ATTEMPTS {
            if attempt > 1 {
                wait(Duration::from_millis(100 * u64::from(attempt - 1)));
                // It may have submitted during backoff. Never send Enter to a fresh composer.
                let screen = self.pane_read(pane, READ_LINES)?;
                anyhow::ensure!(
                    !input_is_collapsed(&screen),
                    "cannot verify collapsed paste after Enter"
                );
                if !input_contains(&screen, text) {
                    delivery.submitted = true;
                    return Ok(());
                }
            }
            delivery.attempts = attempt;
            self.call(&["pane", "send-keys", pane, "enter"])?;
            if self.poll_input(pane, text, false, wait)? {
                delivery.submitted = true;
                return Ok(());
            }
        }
        anyhow::bail!("pane input still contains text after {ENTER_ATTEMPTS} Enter attempts")
    }

    fn poll_input(
        &self,
        pane: &str,
        text: &str,
        present: bool,
        wait: &mut impl FnMut(Duration),
    ) -> Result<bool> {
        for poll in 0..POLLS {
            if poll > 0 {
                wait(POLL_INTERVAL);
            }
            let screen = self.pane_read(pane, READ_LINES)?;
            if !input_is_collapsed(&screen) && input_contains(&screen, text) == present {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Use the last OmO/omp or Claude input marker, including an empty marker. Earlier
/// markers belong to transcript history. Without one, only the bottom nonempty line counts.
pub(crate) fn input_contains(screen: &str, text: &str) -> bool {
    normalize(input_region(screen)).contains(&normalize(text))
}

pub(crate) fn input_is_collapsed(screen: &str) -> bool {
    let input = input_region(screen);
    input.contains("[Pasted text") || input.contains("[paste #")
}

fn input_region(screen: &str) -> &str {
    let mut composer = None;
    let mut offset = 0;
    for line in screen.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed
            .strip_prefix('❯')
            .or_else(|| trimmed.strip_prefix('>'))
            && (rest.is_empty() || rest.starts_with(char::is_whitespace))
        {
            composer = Some(offset + line.len() - rest.len());
        }
        offset += line.len();
    }
    match composer {
        Some(start) => &screen[start..],
        None => screen
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or(""),
    }
}

#[cfg(test)]
mod tests;
