use std::borrow::Cow;

use anyhow::{Context, Result, bail, ensure};
use corral_herdr::{AgentStatus, Herdr};

use super::{LaunchSpec, Origin, Runner, SessionRef, SessionStatus};
use crate::config::{shell_quote, valid_env_key};
use crate::error::refuse;

/// Runs each agent in its own herdr workspace and pane.
#[derive(Clone, Debug)]
pub struct HerdrRunner {
    herdr: Herdr,
}

impl HerdrRunner {
    pub fn new(herdr: Herdr) -> Self {
        Self { herdr }
    }
}

impl Runner for HerdrRunner {
    fn start(&self, spec: &LaunchSpec) -> Result<SessionRef> {
        let command = launch_command(spec)?;
        let created = self
            .herdr
            .workspace_create(&spec.cwd, &spec.label, false)
            .with_context(|| format!("create herdr workspace for {}", spec.label))?;
        let pane = &created.pane_id;
        let tokens: Vec<(&str, &str)> = spec
            .tokens
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let launched = self
            .herdr
            .pane_report_metadata(pane, &spec.label, &tokens)
            .with_context(|| format!("report metadata for herdr pane {pane}"))
            .and_then(|()| {
                self.herdr
                    .pane_run(pane, &command)
                    .with_context(|| format!("launch harness in herdr pane {pane}"))
            });
        if let Err(error) = launched {
            // Closing the only pane also removes its workspace.
            return match self.herdr.pane_close(pane) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!(
                    "also failed to close herdr pane {pane} after launch failure: {cleanup:#}"
                ))),
            };
        }
        Ok(SessionRef::Herdr {
            workspace: created.workspace_id,
            pane: created.pane_id,
        })
    }

    fn status(&self, session: &SessionRef) -> Result<SessionStatus> {
        Ok(self
            .reconciliation_status(session)?
            .unwrap_or(SessionStatus::Idle))
    }

    fn reconciliation_status(&self, session: &SessionRef) -> Result<Option<SessionStatus>> {
        let pane = herdr_pane(session)?;
        let Some(info) = self
            .herdr
            .pane_get(pane)
            .with_context(|| format!("inspect herdr pane {pane}"))?
        else {
            return Ok(Some(SessionStatus::Gone));
        };
        let status = match info.status {
            AgentStatus::Working => SessionStatus::Working,
            AgentStatus::Blocked => {
                let Some(text) = self
                    .herdr
                    .pane_read_if_present(pane, 12)
                    .with_context(|| format!("read blocked herdr pane {pane}"))?
                else {
                    return Ok(Some(SessionStatus::Gone));
                };
                let excerpt = text.trim();
                SessionStatus::Blocked {
                    excerpt: if excerpt.is_empty() {
                        info.title
                            .unwrap_or_else(|| "agent is waiting for input".to_owned())
                    } else {
                        excerpt.to_owned()
                    },
                }
            }
            AgentStatus::Idle | AgentStatus::Done => SessionStatus::Idle,
            AgentStatus::Unknown => return Ok(None),
        };
        Ok(Some(status))
    }

    fn send(&self, session: &SessionRef, text: &str, _origin: Origin) -> Result<()> {
        let pane = herdr_pane(session)?;
        let delivery = self
            .herdr
            .agent_prompt(pane, text)
            .with_context(|| format!("send prompt to herdr pane {pane}"))?;
        if !delivery.submitted {
            bail!(
                "herdr pane {pane} did not confirm prompt submission: {}",
                delivery.error.as_deref().unwrap_or("no delivery details")
            );
        }
        Ok(())
    }

    fn stop(&self, session: &SessionRef) -> Result<()> {
        let pane = herdr_pane(session)?;
        self.herdr
            .pane_close(pane)
            .with_context(|| format!("close herdr pane {pane}"))
    }

    fn focus(&self, session: &SessionRef) -> Result<()> {
        let pane = herdr_pane(session)?;
        self.herdr
            .pane_focus(pane)
            .with_context(|| format!("focus herdr pane {pane}"))
    }
}

fn herdr_pane(session: &SessionRef) -> Result<&str> {
    match session {
        SessionRef::Herdr { pane, .. } => Ok(pane),
        SessionRef::Process { .. } | SessionRef::Pi { .. } => Err(refuse(
            "this session is not owned by the herdr runner; set runner.mode to \"process\" in .corral-next/corral.toml",
            Vec::new(),
        )),
    }
}

fn launch_command(spec: &LaunchSpec) -> Result<Cow<'_, str>> {
    ensure!(!spec.command.contains('\0'), "harness command contains NUL");
    if spec.env.is_empty() {
        return Ok(Cow::Borrowed(&spec.command));
    }
    let env_size = spec
        .env
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum::<usize>();
    let mut command = String::with_capacity(spec.command.len() + env_size + "export && ".len());
    command.push_str("export");
    for (key, value) in &spec.env {
        ensure!(
            valid_env_key(key),
            "invalid session environment key {key:?}"
        );
        let value =
            shell_quote(value).with_context(|| format!("quote session environment {key}"))?;
        command.push(' ');
        command.push_str(key);
        command.push('=');
        command.push_str(&value);
    }
    command.push_str(" && ");
    command.push_str(&spec.command);
    Ok(Cow::Owned(command))
}
