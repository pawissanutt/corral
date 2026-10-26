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
}

pub fn run(ctx: &Ctx, args: AttentionArgs) -> Result<()> {
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

fn next_check(value: &str) -> Result<u64, String> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix('m') {
        (number, 60_u64)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3600)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1)
    } else {
        return Err("next-check requires s, m or h".into());
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(multiplier))
        .filter(|seconds| (60..=86400).contains(seconds))
        .ok_or_else(|| "next-check must be 1m..24h".into())
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
