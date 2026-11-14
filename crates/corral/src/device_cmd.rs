use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum DeviceCmd {
    /// Offer a short-lived browser pairing code and confirm its matching code
    Pair,
    /// List enrolled and revoked app devices
    List {
        #[arg(long)]
        json: bool,
    },
    /// Revoke a device and close its active sessions
    Revoke { device: String },
}

pub fn run(start: &Path, cmd: DeviceCmd, entry: &crate::entry::Entry) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = (start, cmd, entry);
        anyhow::bail!("daemon sockets are unavailable on this platform");
    }
    #[cfg(unix)]
    {
        use corral_ops::daemon_client::Client;
        use corral_protocol::device::DevicePlatform;
        let store = match &cmd {
            DeviceCmd::List { .. } => entry.open(start)?.store,
            DeviceCmd::Pair | DeviceCmd::Revoke { .. } => {
                crate::daemon_cmd::owner_store(start, entry)?
            }
        };
        let Some(mut client) = Client::connect(&store)? else {
            println!("corrald is not running");
            std::process::exit(2);
        };
        match cmd {
            DeviceCmd::Pair => pair(&mut client),
            DeviceCmd::List { json } => {
                let rows = client.devices()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else if rows.is_empty() {
                    println!("No devices.");
                } else {
                    for row in rows {
                        let platform = match row.platform {
                            DevicePlatform::Native => "native",
                            DevicePlatform::Web => "web",
                            DevicePlatform::Other => "other",
                        };
                        let status = row
                            .revoked_at
                            .map_or_else(|| "active".into(), |at| format!("revoked {at}"));
                        println!(
                            "{}  {platform}  {status}  {}  {}",
                            row.id, row.enrolled_at, row.name
                        );
                    }
                }
                Ok(())
            }
            DeviceCmd::Revoke { device } => {
                let info = client.revoke(&device)?;
                println!("Revoked {} \"{}\"", info.id, info.name);
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
fn pair(client: &mut corral_ops::daemon_client::Client) -> Result<()> {
    use anyhow::Context;
    use corral_protocol::device::PairState;
    use std::io::{self, Write};
    use std::time::Duration;

    let offer = client.pair_begin()?;
    println!("Pairing code (expires {}):", offer.expires_at);
    println!("{}", offer.code);
    println!("Paste it in the Corral web app under Settings › Connection.");
    println!("Waiting for a browser…");
    loop {
        match client
            .pair_status(&offer.pair_id, Duration::from_secs(25))?
            .state
        {
            PairState::Offered { .. } => {}
            PairState::Claimed {
                name,
                app_version,
                sas,
                ..
            } => {
                println!("\"{name}\" (web {app_version}) wants to pair.");
                println!("Code shown in the browser: {sas}");
                print!("Pair only if the codes match. Pair? [y/N] ");
                io::stdout().flush().context("flushing pairing prompt")?;
                let mut answer = String::new();
                io::stdin()
                    .read_line(&mut answer)
                    .context("reading pairing confirmation")?;
                if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    let info = client.pair_confirm(&offer.pair_id)?;
                    println!("Paired {} \"{}\"", info.id, info.name);
                    return Ok(());
                }
                client.pair_cancel(&offer.pair_id)?;
                println!("Pairing cancelled");
                std::process::exit(1);
            }
            PairState::Paired { device } => {
                println!("Paired {} \"{}\"", device.id, device.name);
                return Ok(());
            }
            PairState::Ended { reason } => {
                println!("Pairing ended: {reason}");
                std::process::exit(1);
            }
            PairState::Other => anyhow::bail!("corrald returned an unsupported pairing state"),
        }
    }
}
