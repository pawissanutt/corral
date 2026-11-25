//! Dedicated private stdio payload; not a configurable MCP server.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() -> anyhow::Result<()> {
    corral_daemon::run_shim()
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("public Lite fixture requires Linux x86-64")
}
