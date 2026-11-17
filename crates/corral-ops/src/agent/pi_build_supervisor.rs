//! Rich-host profile changes restart under one owner process, preserving the transcript lineage.
use super::*;
use anyhow::ensure;
use std::io::Read;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Request { version: u32, profile: String, session: String, generation: u32 }

pub(super) fn run(args: &BuildArgs, bare: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let workspace = if bare || args.resume.is_some() { None } else { workspace_for(&cwd)? };
    let data_root = super::super::install::data_root()?;
    let runtime = tempfile::Builder::new().prefix("corral-build-profile-").tempdir().context("creating private Build handoff directory")?;
    let request_path = runtime.path().join("profile-request.json");
    let mut next = args.clone();
    loop {
        let (mut child, reader) = command(&data_root, &next, &cwd, find_bun(), workspace.as_deref(), bare, Some(&request_path))?;
        let status = child.status().context("running the Build Pi host")?;
        drop(reader);
        if status.code() != Some(79) {
            ensure!(status.success(), "Build Pi host exited with {status}");
            return Ok(());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&request_path).context("opening Build profile handoff")?.take(4097).read_to_end(&mut bytes).context("reading Build profile handoff")?;
        std::fs::remove_file(&request_path).context("consuming Build profile handoff")?;
        ensure!(bytes.len() <= 4096, "Build profile handoff exceeds bound");
        let request: Request = serde_json::from_slice(&bytes).context("decoding Build profile handoff")?;
        ensure!(request.version == 1 && request.generation == 1 && !request.profile.trim().is_empty() && request.profile.len() <= 128,
            "invalid Build profile handoff");
        let session = resume_session(&data_root, &request.session)?;
        ensure!(session.repo_root == cwd || corral_git::repo_root(&cwd).is_ok_and(|root| root == session.repo_root)
            || next.resume.as_deref() == Some(request.session.as_str()), "Build handoff changed repository");
        next = BuildArgs { resume: Some(request.session), profile: Some(request.profile), ..BuildArgs::default() };
    }
}
