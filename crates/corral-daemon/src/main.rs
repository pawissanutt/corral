#[cfg(unix)]
mod unix {
    use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

    use anyhow::{Context, Result};
    use clap::{Parser, Subcommand};
    use corral_daemon::{RunOptions, client::Client};
    use corral_engine::Engine;
    use corral_protocol::{self as wire, device};
    use serde::Serialize;

    const CONTROL_WAIT: Duration = Duration::from_secs(2);

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
        Pair {
            #[command(subcommand)]
            action: Option<PairAction>,
        },
    }

    #[derive(Subcommand)]
    enum PairAction {
        Status { pair_id: String },
        Confirm { pair_id: String },
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
                    Command::Pair { action } => match action {
                        None => print_json(client.request::<device::PairOffer>(
                            device::PAIR_BEGIN,
                            device::PairBegin {},
                            CONTROL_WAIT,
                        )?),
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
