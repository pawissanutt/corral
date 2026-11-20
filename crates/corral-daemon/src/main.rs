#[cfg(unix)]
mod unix {
    use std::{
        env,
        io::{self, Write},
        net::SocketAddr,
        path::PathBuf,
        time::Duration,
    };

    use anyhow::{Context, Result, bail};
    use clap::{Parser, Subcommand};
    use corral_daemon::{RunOptions, client::Client};
    use corral_engine::Engine;
    use corral_protocol::{self as wire, device};
    use serde::Serialize;

    const CONTROL_WAIT: Duration = Duration::from_secs(2);
    /// The daemon holds a pairing status request open at most this long.
    const PAIR_POLL_MS: u32 = 25_000;

    #[derive(Parser)]
    #[command(version, about = "Serve the simple Corral engine")]
    struct Cli {
        #[arg(long, global = true)]
        root: Option<PathBuf>,
        #[command(subcommand)]
        command: Command,
    }

    #[derive(Subcommand)]
    enum Command {
        Run {
            #[arg(long)]
            listen: Option<SocketAddr>,
            #[arg(long = "web-origin")]
            web_origins: Vec<String>,
            /// Serve a packaged app directory from the app listener
            #[arg(long)]
            web_dir: Option<PathBuf>,
            #[arg(long)]
            web_endpoint: Option<String>,
            #[arg(long)]
            push_key: Option<PathBuf>,
            #[arg(long)]
            push_subject: Option<String>,
            #[arg(long = "push-origin")]
            push_origins: Vec<String>,
        },
        Nudge {
            #[arg(long)]
            wait_ms: Option<u32>,
        },
        Status,
        Stop {
            #[arg(long)]
            drain: bool,
        },
        /// Pair a device: print a link, wait for the device, show its code and ask before enrolling it
        Pair {
            /// Print the offer as JSON and return at once, for scripts
            #[arg(long)]
            json: bool,
            #[command(subcommand)]
            action: Option<PairAction>,
        },
    }

    #[derive(Subcommand)]
    enum PairAction {
        /// Print a pairing's state as JSON
        Status { pair_id: String },
        /// Enrol the device that claimed this pairing
        Confirm { pair_id: String },
        /// End this pairing without enrolling a device
        Cancel { pair_id: String },
    }

    pub(super) fn run() -> Result<()> {
        let cli = Cli::parse();
        let root = Engine::discover_root(
            &env::current_dir().context("read current directory")?,
            cli.root.as_deref(),
        )?;
        match cli.command {
            Command::Run {
                listen,
                web_origins,
                web_dir,
                web_endpoint,
                push_key,
                push_subject,
                push_origins,
            } => {
                if env::var_os("CORRAL_NEXT_BIN").is_none_or(|value| value.is_empty()) {
                    let binary = env::current_exe()
                        .context("resolve corrald-next executable")?
                        .with_file_name("corral-next");
                    // Clap has returned and no daemon or runner threads have started yet.
                    unsafe { env::set_var("CORRAL_NEXT_BIN", binary) };
                }
                corral_daemon::run(
                    &root,
                    RunOptions {
                        listen,
                        web_origins,
                        web_dir,
                        web_endpoint,
                        push_key,
                        push_subject,
                        push_origins,
                    },
                )
            }
            Command::Status => {
                let health: wire::Health = match Client::connect(&root)? {
                    Some(mut client) => {
                        client.request(wire::HEALTH_GET, wire::HealthGet {}, CONTROL_WAIT)?
                    }
                    None => wire::Health::down(),
                };
                println!("{}", health.line());
                Ok(())
            }
            command => {
                let mut client =
                    Client::connect(&root)?.context("daemon is down; run corrald-next run")?;
                match command {
                    Command::Nudge { wait_ms } => print_json(client.request::<wire::NudgeAck>(
                        wire::NUDGE,
                        wire::Nudge {
                            source: "cli".into(),
                            wait_ms,
                        },
                        CONTROL_WAIT
                            + Duration::from_millis(u64::from(wait_ms.unwrap_or_default())),
                    )?),
                    Command::Stop { drain } => {
                        let timeout = if drain {
                            Duration::from_secs(601)
                        } else {
                            CONTROL_WAIT
                        };
                        let _: wire::Stopped =
                            client.request(wire::DAEMON_STOP, wire::Stop { drain }, timeout)?;
                        Ok(())
                    }
                    Command::Pair { action, json } => match action {
                        None if json => print_json(client.request::<device::PairOffer>(
                            device::PAIR_BEGIN,
                            device::PairBegin {},
                            CONTROL_WAIT,
                        )?),
                        None => pair(&mut client),
                        Some(PairAction::Status { pair_id }) => {
                            print_json(client.request::<device::PairStatus>(
                                device::PAIR_STATUS,
                                device::PairStatusQuery {
                                    pair_id,
                                    wait_ms: 0,
                                },
                                CONTROL_WAIT,
                            )?)
                        }
                        Some(PairAction::Confirm { pair_id }) => {
                            print_json(client.request::<device::DeviceInfo>(
                                device::PAIR_CONFIRM,
                                device::PairConfirm { pair_id },
                                CONTROL_WAIT,
                            )?)
                        }
                        Some(PairAction::Cancel { pair_id }) => client.request::<()>(
                            device::PAIR_CANCEL,
                            device::PairCancel { pair_id },
                            CONTROL_WAIT,
                        ),
                    },
                    Command::Run { .. } | Command::Status => unreachable!("already handled"),
                }
            }
        }
    }

    /// The owner's side of pairing: offer a link, wait for the device to claim it, show the
    /// device's code and enrol it only on a yes.
    fn pair(client: &mut Client) -> Result<()> {
        let offer: device::PairOffer =
            client.request(device::PAIR_BEGIN, device::PairBegin {}, CONTROL_WAIT)?;
        let until = offer.expires_at.parse::<jiff::Timestamp>().map_or_else(
            |_| offer.expires_at.clone(),
            |at| {
                at.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%H:%M:%S")
                    .to_string()
            },
        );
        match &offer.url {
            Some(url) => {
                println!("Pairing link, valid until {until}:\n{url}");
                println!("Open it in the browser you want to pair and press Pair there.");
            }
            None => {
                println!("Pairing code, valid until {until}:\n{}", offer.code);
                println!("Paste it in the Corral app under Settings › Connection and press Pair.");
            }
        }
        println!("Waiting for the device…");
        loop {
            let status: device::PairStatus = client.request(
                device::PAIR_STATUS,
                device::PairStatusQuery {
                    pair_id: offer.pair_id.clone(),
                    wait_ms: PAIR_POLL_MS,
                },
                CONTROL_WAIT + Duration::from_millis(PAIR_POLL_MS.into()),
            )?;
            match status.state {
                device::PairState::Offered { .. } => {}
                device::PairState::Claimed {
                    name,
                    app_version,
                    sas,
                    ..
                } => {
                    println!("\"{name}\" (app {app_version}) wants to pair.");
                    println!("Its code: {sas}");
                    print!("Pair only if the device shows the same code. Pair? [y/N] ");
                    io::stdout().flush().context("show the pairing question")?;
                    let mut answer = String::new();
                    io::stdin()
                        .read_line(&mut answer)
                        .context("read the pairing answer")?;
                    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                        client.request::<()>(
                            device::PAIR_CANCEL,
                            device::PairCancel {
                                pair_id: offer.pair_id,
                            },
                            CONTROL_WAIT,
                        )?;
                        bail!("pairing cancelled; no device was enrolled");
                    }
                    let device: device::DeviceInfo = client.request(
                        device::PAIR_CONFIRM,
                        device::PairConfirm {
                            pair_id: offer.pair_id,
                        },
                        CONTROL_WAIT,
                    )?;
                    println!("Paired {} \"{}\"", device.id, device.name);
                    return Ok(());
                }
                device::PairState::Paired { device } => {
                    println!("Paired {} \"{}\"", device.id, device.name);
                    return Ok(());
                }
                device::PairState::Ended { reason } => bail!("pairing ended: {reason}"),
                device::PairState::Other => {
                    bail!("the daemon reported a pairing state this command does not know")
                }
            }
        }
    }

    fn print_json(value: impl Serialize) -> Result<()> {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).context("encode daemon result")?
        );
        Ok(())
    }
}

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    unix::run()
}

#[cfg(not(unix))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("corrald-next requires Unix sockets")
}
