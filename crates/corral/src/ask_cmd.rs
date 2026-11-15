use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Args;

#[derive(Args)]
pub struct AskArgs {
    /// Send to the PM inbox instead of the assistant quick thread
    #[arg(long)]
    pm: bool,
    /// Attach an image or UTF-8 file (assistant only; repeat for multiple files)
    #[arg(long, value_name = "PATH")]
    file: Vec<PathBuf>,
    /// Message text, or - to read it from standard input
    #[arg(value_name = "TEXT|-")]
    text: String,
}

pub fn run(start: &Path, args: AskArgs, entry: &crate::entry::Entry) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = (start, args, entry);
        anyhow::bail!("daemon sockets are unavailable on this platform")
    }
    #[cfg(unix)]
    {
        use anyhow::{Context, ensure};
        use corral_protocol::assistant::{
            AskTarget, AssistantSend, MESSAGE_TEXT_MAX, QUICK_THREAD,
        };
        use corral_protocol::attachment::ATTACHMENTS_PER_MESSAGE;
        use std::io::Read;

        ensure!(
            !args.pm || args.file.is_empty(),
            "PM messages do not support attachments"
        );
        ensure!(
            args.file.len() <= ATTACHMENTS_PER_MESSAGE,
            "at most {ATTACHMENTS_PER_MESSAGE} attachments are supported"
        );
        let text = if args.text == "-" {
            let mut text = String::new();
            std::io::stdin()
                .take((MESSAGE_TEXT_MAX + 1) as u64)
                .read_to_string(&mut text)
                .context("reading message from stdin")?;
            text
        } else {
            args.text
        };
        ensure!(
            !text.trim().is_empty() && text.len() <= MESSAGE_TEXT_MAX,
            "message must contain 1 to {MESSAGE_TEXT_MAX} bytes"
        );
        let store = crate::daemon_cmd::owner_store(start, entry)?;
        let mut client = corral_ops::daemon_client::AppClient::connect(&store)
            .context("connecting to corrald for Ask")?;
        let mut attachments = Vec::with_capacity(args.file.len());
        for path in &args.file {
            attachments.push(
                client
                    .upload(path)
                    .with_context(|| format!("uploading {}", path.display()))?,
            );
        }
        let request = AssistantSend {
            cmd_id: corral_ops::daemon_client::new_app_command_id()?,
            thread: QUICK_THREAD.into(),
            target: if args.pm {
                AskTarget::Pm
            } else {
                AskTarget::Assistant
            },
            text,
            typed_at: jiff::Timestamp::now().to_string(),
            context: Vec::new(),
            attachments,
        };
        let ack = client.send(&request)?;
        println!("{}", ack.entry);
        Ok(())
    }
}
