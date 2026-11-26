//! One-shot Pi host reads. Poll outside request dispatch so slow providers cannot delay saves.
use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::Result;
use corral_engine::pi_hosted_tools::valid_model_id;
use corral_protocol::{
    app::APP_CONTROL_MAX,
    config::{
        PiHostedTool, PiHostedToolModels, PiHostedToolModelsRequest, PiHostedToolModelsStatus,
        PiHostedToolProvider,
    },
};
use serde_json::Value;

use crate::conn::{Conn, reply};

const DEADLINE: Duration = Duration::from_secs(15);
const MAX_PENDING: usize = 4;
const MAX_OUTPUT: u64 = (APP_CONTROL_MAX - 4096) as u64;
const MAX_MODELS: usize = 512;

#[derive(Default)]
pub(crate) struct Pending(Vec<(Value, Attempt)>);

impl Pending {
    pub(crate) fn start(
        &mut self,
        conn: &mut dyn Conn,
        id: Value,
        root: &Path,
        request: PiHostedToolModelsRequest,
    ) -> Result<()> {
        if self.0.len() < MAX_PENDING
            && let Some(attempt) = Attempt::start(root, request)
        {
            self.0.push((id, attempt));
            return Ok(());
        }
        reply(conn, id, failed())
    }

    pub(crate) fn flush(&mut self, conn: &mut dyn Conn) -> Result<()> {
        let mut i = 0;
        while i < self.0.len() {
            if let Some(result) = self.0[i].1.poll() {
                let (id, _) = self.0.swap_remove(i);
                reply(conn, id, result)?;
            } else {
                i += 1;
            }
        }
        Ok(())
    }
}

struct Attempt {
    child: Child,
    output: File,
    provider: &'static str,
    started: Instant,
}

impl Attempt {
    fn start(root: &Path, request: PiHostedToolModelsRequest) -> Option<Self> {
        let tool = match request.tool {
            PiHostedTool::WebSearch => "web_search",
            PiHostedTool::ImageGen => "image_gen",
        };
        let provider = match request.provider {
            PiHostedToolProvider::OpenAi => "openai",
            PiHostedToolProvider::OpenAiCodex => "openai-codex",
            PiHostedToolProvider::Anthropic => "anthropic",
            PiHostedToolProvider::AnthropicApi => "anthropic-api",
        };
        // The private, unnamed output avoids a pipe write blocking discovery when the
        // connection is handling another request. Never retain or forward host stderr.
        let output = tempfile::tempfile().ok()?;
        let child = Command::new("corral-pi")
            .arg("hosted-tool-models")
            .arg("--root")
            .arg(root)
            .args(["--tool", tool, "--provider", provider])
            .current_dir(root)
            .env("CORRAL_NEXT_ROOT", root)
            .stdin(Stdio::null())
            .stdout(output.try_clone().ok()?)
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        Some(Self {
            child,
            output,
            provider,
            started: Instant::now(),
        })
    }

    fn poll(&mut self) -> Option<PiHostedToolModels> {
        if self.started.elapsed() >= DEADLINE
            || !self.output.metadata().is_ok_and(|m| m.len() <= MAX_OUTPUT)
        {
            return Some(failed());
        }
        match self.child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) if status.success() => {
                Some(read_models(&mut self.output, self.provider).unwrap_or_else(failed))
            }
            _ => Some(failed()),
        }
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        // Also reap on disconnect, overflow and timeout; this CLI replaces its launcher
        // process and does not spawn a provider subprocess.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_models(output: &mut File, provider: &str) -> Option<PiHostedToolModels> {
    output.rewind().ok()?;
    let mut bytes = Vec::new();
    output.take(MAX_OUTPUT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_OUTPUT {
        return None;
    }
    let result: PiHostedToolModels = serde_json::from_slice(&bytes).ok()?;
    (result.models.len() <= MAX_MODELS
        && (result.status == PiHostedToolModelsStatus::Live || result.models.is_empty())
        && result.models.iter().all(|id| valid_model_id(provider, id)))
    .then_some(result)
}

fn failed() -> PiHostedToolModels {
    PiHostedToolModels {
        status: PiHostedToolModelsStatus::Failed,
        models: vec![],
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn host_output_is_only_bounded_selector_metadata() -> Result<()> {
        let read = |bytes: &[u8]| -> Result<_> {
            let mut output = tempfile::tempfile()?;
            output.write_all(bytes)?;
            Ok(read_models(&mut output, "openai"))
        };
        let result = read(br#"{"status":"live","models":["image-fixture"]}"#)?;
        assert_eq!(result.map(|r| r.models), Some(vec!["image-fixture".into()]));
        for invalid in [
            &br#"{"status":"live","models":["sk-secret-fixture"]}"#[..],
            br#"{"status":"live","models":[],"error":"secret-fixture"}"#,
            br#"{"status":"failed","models":["image-fixture"]}"#,
        ] {
            assert!(read(invalid)?.is_none());
        }
        assert!(read(&vec![b' '; MAX_OUTPUT as usize + 1])?.is_none());
        Ok(())
    }
}
