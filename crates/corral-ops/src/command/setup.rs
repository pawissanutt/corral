//! `setup` command executors.
use anyhow::{Context, Result, ensure};
use corral_core::{AutoProfileSelection, ProfileSetupApproval};
use corral_protocol::cmd::{FileInput, setup::SetupCommand};
use serde::de::DeserializeOwned;

use super::{Exec, OpenSpec, Verb, render::ErrorStyle};

impl Verb for SetupCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        let store = &cx.ctx.store;
        let max = store.config.artifacts.max_bytes;
        match self {
            Self::Begin { expected_revision } => cx
                .out
                .json_pretty(&crate::setup_begin(store, expected_revision)?),
            Self::Answers {
                file,
                expected_revision,
            } => {
                let answers = read_input(&file, max)?;
                cx.out
                    .json_pretty(&crate::setup_answers(store, &answers, expected_revision)?)
            }
            Self::Propose {
                file,
                expected_revision,
            } => {
                check_size(&file, max)?;
                let input = crate::parse_setup_input(file.text.as_bytes())
                    .with_context(|| format!("validating setup input {}", file.path))?;
                cx.out.json_pretty(&crate::setup_propose_draft(
                    store,
                    &input,
                    expected_revision,
                )?)
            }
            Self::Approve {
                file,
                expected_revision,
            } => {
                let input: ProfileSetupApproval = read_input(&file, max)?;
                cx.out
                    .json_pretty(&crate::setup_approve(store, &input, expected_revision)?)
            }
            Self::Cancel { expected_revision } => cx
                .out
                .json_pretty(&crate::setup_cancel(store, expected_revision)?),
            Self::SelectionSave { file } => {
                let receipt: AutoProfileSelection = read_input(&file, max)?;
                cx.out.json_pretty(&crate::selection_save(store, &receipt)?)
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }

    fn open(&self) -> OpenSpec {
        OpenSpec::UNOBSERVED
    }

    fn error_style(&self) -> ErrorStyle {
        match self {
            Self::SelectionSave { .. } => ErrorStyle::Family {
                command: "selection save".into(),
            },
            _ => ErrorStyle::Setup {
                command: format!("setup {}", self.verb()),
            },
        }
    }
}

fn check_size(file: &FileInput, max: u64) -> Result<()> {
    ensure!(
        u64::try_from(file.text.len())? <= max,
        "{} exceeds artifacts.max_bytes ({max} bytes)",
        file.path
    );
    Ok(())
}

fn read_input<T: DeserializeOwned>(file: &FileInput, max: u64) -> Result<T> {
    check_size(file, max)?;
    serde_json::from_slice(file.text.as_bytes())
        .with_context(|| format!("parsing typed JSON input {}", file.path))
}
