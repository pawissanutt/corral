use anyhow::Result;
use corral_core::Status;
use corral_ops::Ctx;

#[derive(clap::Args)]
pub struct AttentionArgs {
    #[command(subcommand)]
    pub cmd: Option<AttentionCmd>,
    /// Print the versioned assessment, including tasks without conditions
    #[arg(long, global = true)]
    pub json: bool,
    /// Include Done and Dropped tasks
    #[arg(long, global = true)]
    pub all: bool,
    /// Include only tasks with this exact project name
    #[arg(long, global = true)]
    pub project: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum AttentionCmd {
    /// List the current read-only attention assessment (the default)
    List,
    /// Record handling without answering, moving or authorizing the task
    Attend {
        key: String,
        #[arg(long)]
        note: String,
        #[arg(long, default_value = "30m", value_parser = next_check)]
        next_check: u64,
        #[arg(long)]
        request: Option<String>,
    },
    /// Stop new execution while retaining explicit, checkable resume conditions
    Park {
        id: String,
        #[arg(long)]
        reason: String,
        #[arg(long, value_parser = corral_core::parse_park_check)]
        resume_when: Vec<corral_core::ResumeCheck>,
        #[arg(long, default_value = "")]
        resume_action: String,
        #[arg(long, default_value = "24h", value_parser = review_by)]
        review_by: u64,
    },
    /// Remove exactly the current park; never lifts a dispatch hold
    Unpark {
        id: String,
        #[arg(long)]
        expected_revision: u64,
    },
}

pub fn run(ctx: &Ctx, args: AttentionArgs) -> Result<()> {
    match &args.cmd {
        Some(AttentionCmd::Park {
            id,
            reason,
            resume_when,
            resume_action,
            review_by,
        }) => {
            let park = corral_ops::park::park(
                ctx,
                id,
                reason,
                resume_when,
                resume_action,
                *review_by,
                jiff::Timestamp::now(),
            )?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&park)?);
            } else {
                println!(
                    "{id}: parked by {} at revision {}, review by {}",
                    park.by.label(),
                    park.revision,
                    park.review_by
                );
            }
            return Ok(());
        }
        Some(AttentionCmd::Unpark {
            id,
            expected_revision,
        }) => {
            corral_ops::park::unpark(ctx, id, *expected_revision)?;
            if args.json {
                println!(
                    "{}",
                    serde_json::json!({"task": id, "unparked": expected_revision})
                );
            } else {
                println!("{id}: unparked revision {expected_revision}");
            }
            return Ok(());
        }
        Some(AttentionCmd::List | AttentionCmd::Attend { .. }) | None => {}
    }
    if let Some(AttentionCmd::Attend {
        key,
        note,
        next_check,
        request,
    }) = args.cmd
    {
        let now = jiff::Timestamp::now();
        let request = request.unwrap_or_else(|| format!("attend-{now}"));
        let handling = corral_ops::attention::attend(ctx, &key, &note, next_check, &request, now)?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&handling)?);
        } else {
            println!(
                "{key}: handling revision {}, next check {}",
                handling.revision, handling.next_check
            );
        }
        return Ok(());
    }
    let mut board = corral_ops::attention::board(ctx, jiff::Timestamp::now())?;
    board.tasks.retain(|task| {
        (args.all || !matches!(task.status, Status::Done | Status::Dropped))
            && args
                .project
                .as_ref()
                .is_none_or(|project| task.project.as_ref() == Some(project))
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&board)?);
        return Ok(());
    }
    let mut rows = 0;
    for task in &board.tasks {
        if task.assessment.conditions.is_empty() {
            continue;
        }
        if let Some(primary) = &task.assessment.primary {
            let age = match primary.age_secs {
                Some(seconds) => format!("{seconds}s ({})", primary.since_source.label()),
                None => format!("unknown ({})", primary.since_source.label()),
            };
            let next = &primary.next_action;
            let action = match &next.command {
                Some(command) => format!("{command}: {}", next.description),
                None => next.description.clone(),
            };
            println!(
                "{}  {}  {}  {}  age {}  {}  next: {}",
                task.task,
                one_line(&task.title),
                primary.class.label(),
                primary.responsible.label(),
                age,
                one_line(&primary.reason),
                one_line(&action),
            );
            rows += 1;
        }
    }
    if rows == 0 {
        println!("No attention conditions.");
    }
    Ok(())
}

fn duration(
    value: &str,
    key: &str,
    range: std::ops::RangeInclusive<u64>,
    interval: &str,
) -> Result<u64, String> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix('m') {
        (number, 60_u64)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3600)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1)
    } else if let Some(number) = value.strip_suffix('d') {
        (number, 86400)
    } else {
        return Err(format!("{key} requires s, m, h or d"));
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(multiplier))
        .filter(|seconds| range.contains(seconds))
        .ok_or_else(|| format!("{key} must be {interval}"))
}

fn next_check(value: &str) -> Result<u64, String> {
    duration(value, "next-check", 60..=86400, "1m..24h")
}

fn review_by(value: &str) -> Result<u64, String> {
    duration(value, "review-by", 3600..=604800, "1h..7d")
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn park_cli_parses_checks_and_refuses_out_of_range_review_deadlines() {
        // Given the public park syntax and adopted review interval.
        for (deadline, expected) in [
            ("59m", None),
            ("1h", Some(3600)),
            ("7d", Some(604800)),
            ("8d", None),
        ] {
            let parsed = crate::Cli::try_parse_from([
                "corral",
                "attention",
                "park",
                "T-128",
                "--reason",
                "Wait",
                "--resume-when",
                "landed:T-371",
                "--resume-when",
                "status:T-372=done",
                "--resume-action",
                "Review findings",
                "--review-by",
                deadline,
            ]);
            // When Clap parses the request, then machine fields match or parsing refuses.
            match expected {
                Some(seconds) => match parsed.unwrap().cmd {
                    Some(crate::Cmd::Attention(AttentionArgs {
                        cmd:
                            Some(AttentionCmd::Park {
                                resume_when,
                                review_by,
                                ..
                            }),
                        ..
                    })) => {
                        assert_eq!(review_by, seconds);
                        assert_eq!(resume_when.len(), 2);
                        assert_eq!(resume_when[0].kind, "landed");
                        assert_eq!(resume_when[1].expected.as_deref(), Some("done"));
                    }
                    _ => panic!("expected park"),
                },
                None => assert!(parsed.is_err()),
            }
        }
    }

    #[test]
    fn park_command_executes_and_worker_contexts_refuse_without_writes() {
        // Given a fresh task and a subprocess-isolated caller context.
        let tmp = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(tmp.path()).unwrap();
        let task = store.create("Park CLI", None, Status::Ready, "").unwrap();
        for role in ["worker", "reviewer", "pm", "unpark"] {
            let before = store.get(&task.id).unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "attention_cmd::tests::park_command_subprocess",
                    "--nocapture",
                ])
                .env("PARK_CLI_ROOT", tmp.path())
                .env("PARK_CLI_TASK", &task.id)
                .env("PARK_CLI_ROLE", role)
                .current_dir(tmp.path());
            for key in [
                "AI_AGENT",
                "PI_CODING_AGENT",
                "CORRAL_AGENT",
                "CORRAL_REVIEWER",
                "CORRAL_AGENT_TASK",
                "CORRAL_PM",
            ] {
                command.env_remove(key);
            }
            match role {
                "worker" => {
                    command.env("AI_AGENT", "1");
                }
                "reviewer" => {
                    command.env("CORRAL_REVIEWER", "1");
                }
                "pm" | "unpark" => {
                    command.env("CORRAL_PM", "1").env("AI_AGENT", "1");
                }
                _ => unreachable!(),
            }
            // When the actual parsed subcommand executes in that process.
            let output = command.stdin(std::process::Stdio::null()).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            // Then only the authorized PM command can change the task.
            let after = store.get(&task.id).unwrap();
            match role {
                "worker" | "reviewer" => assert_eq!(after, before),
                "pm" => assert_eq!(after.park.unwrap().by, corral_core::Responsible::Pm),
                "unpark" => assert!(after.park.is_none()),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn park_command_subprocess() {
        let Ok(root) = std::env::var("PARK_CLI_ROOT") else {
            return;
        };
        let id = std::env::var("PARK_CLI_TASK").unwrap();
        let role = std::env::var("PARK_CLI_ROLE").unwrap();
        let ctx = Ctx::from_store(corral_core::Store::open(std::path::Path::new(&root)).unwrap());
        let mut args = vec!["corral".to_owned(), "attention".into()];
        if role == "unpark" {
            let revision = ctx.store.get(&id).unwrap().park.unwrap().revision;
            args.extend([
                "unpark".into(),
                id,
                "--expected-revision".into(),
                revision.to_string(),
            ]);
        } else {
            args.extend([
                "park".into(),
                id,
                "--reason".into(),
                "Wait".into(),
                "--review-by".into(),
                "24h".into(),
            ]);
        }
        let Some(crate::Cmd::Attention(args)) = crate::Cli::try_parse_from(args).unwrap().cmd
        else {
            panic!("expected attention")
        };
        let result = run(&ctx, args);
        if matches!(role.as_str(), "worker" | "reviewer") {
            assert!(result.unwrap_err().to_string().contains("worker_context"));
        } else {
            result.unwrap();
        }
    }

    #[test]
    fn attend_parses_duration_and_refuses_values_outside_the_adopted_interval() {
        // Given a public CLI invocation, not a hand-built argument structure.
        let args = [
            "corral",
            "attention",
            "attend",
            "T-001/question_open/Q1@1",
            "--note",
            "Investigating",
        ];
        for (duration, expected) in [
            ("59s", None),
            ("1m", Some(60)),
            ("30m", Some(1800)),
            ("24h", Some(86400)),
            ("25h", None),
        ] {
            let parsed =
                crate::Cli::try_parse_from(args.into_iter().chain(["--next-check", duration]));
            // When Clap parses it, then the machine duration or refusal is exact.
            match expected {
                Some(expected) => match parsed.unwrap().cmd {
                    Some(crate::Cmd::Attention(AttentionArgs {
                        cmd: Some(AttentionCmd::Attend { next_check, .. }),
                        ..
                    })) => assert_eq!(next_check, expected),
                    _ => panic!("expected attention attend"),
                },
                None => assert!(parsed.is_err(), "{duration}"),
            }
        }
    }
}
