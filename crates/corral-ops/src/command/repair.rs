//! Store-only repair mutations; file contents and owner identity come from the caller.
use anyhow::{Context, Result};
use corral_core::{RepairPolicy, RepairRenewal};
use corral_protocol::cmd::{FileInput, repair::RepairCommand};
use serde::{Deserialize, de::DeserializeOwned};

use super::{Exec, OpenSpec, Verb, render::ErrorStyle};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiagnosisResult {
    diagnosis: String,
    proposal: String,
    evidence: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Renewal {
    renewal: RepairRenewal,
    policy: RepairPolicy,
}

fn read<T: DeserializeOwned>(file: &FileInput) -> Result<T> {
    serde_json::from_str(&file.text).with_context(|| format!("decoding {}", file.path))
}

impl Verb for RepairCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        let value = match self {
            Self::Triage { id, file, owner } => serde_json::to_value(
                crate::gate_triage::diagnose(&cx.ctx, &id, read(&file)?, owner, cx.invoker)?,
            )?,
            Self::Finding { id, file } => {
                serde_json::to_value(crate::repair_finding(&cx.ctx, &id, read(&file)?)?)?
            }
            Self::Diagnose { id, owner } => serde_json::to_value(if owner {
                crate::repair_diagnose_owner(&cx.ctx, &id, cx.invoker)?
            } else {
                crate::repair_diagnose(&cx.ctx, &id)?
            })?,
            Self::Diagnosed { id, file } => {
                let result: DiagnosisResult = read(&file)?;
                serde_json::to_value(crate::repair_diagnosed(
                    &cx.ctx,
                    &id,
                    &result.diagnosis,
                    &result.proposal,
                    result.evidence,
                )?)?
            }
            Self::Renew { id, file } => {
                crate::require_automation_owner(&cx.ctx.store, cx.invoker)?;
                let renewal: Renewal = read(&file)?;
                serde_json::to_value(crate::repair_renew(
                    &cx.ctx,
                    &id,
                    renewal.renewal,
                    renewal.policy,
                    cx.invoker,
                )?)?
            }
            Self::Other => return Err(super::unknown_command("other")),
        };
        cx.out.json_pretty(&value)
    }

    fn open(&self) -> OpenSpec {
        OpenSpec::UNOBSERVED_APPLIED
    }

    fn error_style(&self) -> ErrorStyle {
        ErrorStyle::Family {
            command: format!("repair {}", self.verb()),
        }
    }
}
