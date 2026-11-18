use std::{
    borrow::Cow,
    io::{self, StdoutLock, Write},
    path::Path,
};

use anyhow::{Context, Result};
use corral_engine::{
    Actor, Applied, Artifact, Check, Event, Next, Run, Task, UsageSummary, UsageTotal, as_refusal,
};
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

pub(crate) struct Output<'a> {
    pub program: &'a str,
    pub root: Option<&'a Path>,
    pub json: bool,
    pub stdout: StdoutLock<'static>,
}

impl Output<'_> {
    pub fn value<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
        human: impl FnOnce(&mut StdoutLock<'static>, &T) -> io::Result<()>,
    ) -> Result<()> {
        if self.json {
            serde_json::to_writer(&mut self.stdout, value).context("write JSON output")?;
            writeln!(self.stdout).context("finish JSON output")
        } else {
            human(&mut self.stdout, value).context("write command output")
        }
    }

    pub fn report(&self, error: &anyhow::Error) -> u8 {
        if let Some(refusal) = as_refusal(error) {
            eprintln!("refused: {}", refusal.reason);
            for next in &refusal.next {
                eprintln!("next: {}", next_command(self.program, self.root, next));
            }
            2
        } else {
            eprintln!("error: {error:#}");
            1
        }
    }

    pub fn applied(&mut self, applied: &Applied) -> Result<()> {
        self.value(applied, |out, applied| {
            if let Some(task) = &applied.task {
                writeln!(
                    out,
                    "{}: {} — {}",
                    task.id,
                    task.status.as_str(),
                    task.title
                )?;
            }
            if let Some(run) = &applied.run {
                writeln!(
                    out,
                    "{} {}: {}",
                    run.role.as_str(),
                    run.id,
                    run.worktree.display()
                )?;
            }
            if let Some(artifact) = &applied.artifact {
                artifact_line(out, artifact)?;
            }
            Ok(())
        })
    }

    pub fn show(&mut self, view: &TaskView) -> Result<()> {
        let (program, root) = (self.program, self.root);
        self.value(view, |out, view| {
            let TaskView {
                task,
                runs,
                checks,
                artifacts,
            } = view;
            writeln!(out, "{}: {}", task.id, task.title)?;
            writeln!(
                out,
                "Status: {}    Priority: {:?}    Round: {}",
                task.status.as_str(),
                task.priority,
                task.round
            )?;
            for (label, value) in [
                ("Project", &task.project),
                ("Kind", &task.kind),
                ("Size", &task.size),
                ("Profile", &task.profile),
                ("Parent", &task.parent),
            ] {
                if let Some(value) = value {
                    writeln!(out, "{label}: {value}")?;
                }
            }
            if !task.depends_on.is_empty() {
                writeln!(out, "Depends on: {}", task.depends_on.join(", "))?;
            }
            writeln!(
                out,
                "Created: {}    Updated: {}",
                task.created, task.updated
            )?;
            if let Some(blocked) = &task.blocked {
                writeln!(out, "\nBlocked: {}", blocked.reason)?;
                if let Some(detail) = &blocked.detail {
                    writeln!(out, "{detail}")?;
                }
                for next in &blocked.next {
                    writeln!(out, "next: {}", next_command(program, root, next))?;
                }
            }
            if !task.body.is_empty() {
                writeln!(out, "\n{}", task.body)?;
            }
            writeln!(out, "\nRuns:")?;
            for run in runs {
                writeln!(
                    out,
                    "  {} {}: {} ({})",
                    run.id,
                    run.role.as_str(),
                    run.profile,
                    run.branch
                )?;
                match run.outcome {
                    Some(outcome) => writeln!(out, "    {} — {:?}", run.started, outcome)?,
                    None => writeln!(out, "    {} — live", run.started)?,
                }
                if let Some(summary) = &run.summary {
                    writeln!(out, "    {summary}")?;
                }
            }
            writeln!(out, "\nChecks:")?;
            for check in checks {
                writeln!(
                    out,
                    "  {} {}: {:?} at {}",
                    check.kind.as_str(),
                    check.key,
                    check.result,
                    check.at
                )?;
                if let Some(notes) = &check.notes {
                    writeln!(out, "    {notes}")?;
                }
                if let Some(log) = &check.log {
                    writeln!(out, "    log: {log}")?;
                }
            }
            writeln!(out, "\nArtifacts:")?;
            artifacts
                .iter()
                .try_for_each(|artifact| artifact_line(out, artifact))
        })
    }
}

fn next_command(program: &str, root: Option<&Path>, next: &Next) -> String {
    if let Some(root) = root {
        let arg = format!("--root={}", root.display());
        next.command_with_args(program, &[&arg])
    } else {
        next.command_with_program(program)
    }
}

#[derive(Serialize)]
pub(crate) struct TaskView {
    pub task: Task,
    pub runs: Vec<Run>,
    pub checks: Vec<Check>,
    pub artifacts: Vec<Artifact>,
}

pub(crate) fn tasks(out: &mut impl Write, tasks: &[Task]) -> io::Result<()> {
    table(
        out,
        ["ID", "STATUS", "PRI", "PROJECT", "TITLE"],
        tasks.iter().map(|task| {
            [
                task.id.as_str().into(),
                task.status.as_str().into(),
                task.priority.as_str().into(),
                task.project.as_deref().unwrap_or("-").into(),
                if task.title.contains(['\n', '\r']) {
                    task.title.replace(['\n', '\r'], " ").into()
                } else {
                    task.title.as_str().into()
                },
            ]
        }),
    )
}

pub(crate) fn log(out: &mut impl Write, events: &[Event]) -> io::Result<()> {
    table(
        out,
        ["SEQ", "TIME", "KIND", "ACTOR", "RUN", "DETAIL"],
        events.iter().map(|event| {
            [
                event.seq.to_string().into(),
                event.ts.to_string().into(),
                event.kind.as_str().into(),
                actor(event.actor).into(),
                event.run.as_deref().unwrap_or("-").into(),
                event.detail.to_string().into(),
            ]
        }),
    )
}

pub(crate) fn event(out: &mut impl Write, event: &Event) -> io::Result<()> {
    writeln!(
        out,
        "{} {} {} {} {} {} {}",
        event.seq,
        event.ts,
        event.kind.as_str(),
        actor(event.actor),
        event.task.as_deref().unwrap_or("-"),
        event.run.as_deref().unwrap_or("-"),
        event.detail
    )
}

fn actor(actor: Actor) -> &'static str {
    match actor {
        Actor::Owner => "owner",
        Actor::Agent => "agent",
        Actor::Reviewer => "reviewer",
        Actor::Assistant => "assistant",
        Actor::Auto => "auto",
    }
}

fn table<'a, const N: usize>(
    out: &mut impl Write,
    headers: [&str; N],
    rows: impl Iterator<Item = [Cow<'a, str>; N]>,
) -> io::Result<()> {
    let mut widths = headers.map(UnicodeWidthStr::width);
    let rows: Vec<_> = rows
        .inspect(|row| {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.width());
            }
        })
        .collect();
    table_row(out, &widths, headers)?;
    for row in &rows {
        table_row(out, &widths, row.each_ref().map(|cell| cell.as_ref()))?;
    }
    Ok(())
}

fn table_row<const N: usize>(
    out: &mut impl Write,
    widths: &[usize; N],
    row: [&str; N],
) -> io::Result<()> {
    for (index, (cell, width)) in row.into_iter().zip(widths).enumerate() {
        write!(out, "{cell}")?;
        if index + 1 < N {
            let spaces = width - cell.width() + 2;
            write!(out, "{:spaces$}", "")?;
        }
    }
    writeln!(out)
}

pub(crate) fn artifact_line(out: &mut impl Write, artifact: &Artifact) -> io::Result<()> {
    writeln!(
        out,
        "  {}:{}  {} B  {}  {}",
        artifact.task,
        artifact.id,
        artifact.bytes,
        artifact.kind.as_deref().unwrap_or("-"),
        artifact.title
    )
}

pub(crate) fn usage(out: &mut impl Write, summary: &UsageSummary) -> io::Result<()> {
    let missing = summary.total_runs - summary.covered_runs;
    writeln!(
        out,
        "Reported usage covers {} of {} runs ({} runs have no matching reports).",
        summary.covered_runs, summary.total_runs, missing
    )?;
    if missing > 0 {
        writeln!(
            out,
            "Runs whose harness did not report usage are not estimated."
        )?;
    }
    writeln!(
        out,
        "Totals include all recorded requests, including those without a matching run; costs only include priced requests."
    )?;
    usage_line(out, "Total", &summary.total)?;
    for group in &summary.groups {
        usage_line(
            out,
            group.key.as_deref().unwrap_or("(unassigned)"),
            &group.total,
        )?;
    }
    Ok(())
}

fn usage_line(out: &mut impl Write, label: &str, total: &UsageTotal) -> io::Result<()> {
    write!(
        out,
        "{label}: {} requests, {} input, {} output, {} cache read, {} cache write",
        total.requests, total.input, total.output, total.cache_read, total.cache_write
    )?;
    if let Some(usd) = total.usd {
        write!(
            out,
            ", ${usd:.6} across {} priced requests",
            total.priced_requests
        )?;
    }
    writeln!(out)
}
