//! Owner-only CLI adapter for the shared annotation service and bounded read contract.
use std::{fs::File, io::Read, path::Path, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_core::Store;
use corral_ops::annotations::{self, *};
use serde_json::{Value, json};

#[derive(Args)]
pub struct AnnotationArgs {
    /// Emit the shared v1 view/result; refusals are JSON on stderr
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub cmd: AnnotationCmd,
}

#[derive(Subcommand)]
pub enum AnnotationCmd {
    /// Open a thread on the exact committed candidate you observed
    Open {
        task: String,
        #[arg(long)]
        run: String,
        #[arg(long)]
        base: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        path: String,
        /// Anchor to the base (old) side instead of the head (new) side
        #[arg(long)]
        old: bool,
        /// 1-based inclusive range A-B (omit for a file-level anchor)
        #[arg(long)]
        lines: Option<String>,
        #[command(flatten)]
        note: NoteArgs,
    },
    /// Reply without changing the thread's open/resolved state
    Reply(ChangeArgs),
    /// Explicitly resolve an open thread, with an optional note
    Resolve(ChangeArgs),
    /// Explicitly reopen a resolved thread, with an optional note
    Reopen(ChangeArgs),
    /// Read one bounded page of thread summaries, without entries
    List {
        task: String,
        #[arg(long, value_parser = ["open", "resolved"])]
        state: Option<String>,
        #[arg(long, value_parser = ["current", "stale", "superseded", "unavailable"])]
        anchor: Option<String>,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Read one bounded page of immutable entries in a thread
    Show {
        task: String,
        thread: String,
        #[command(flatten)]
        page: PageArgs,
    },
}

#[derive(Args)]
pub struct ChangeArgs {
    task: String,
    thread: String,
    /// Thread revision from the read view (required even for state transitions)
    #[arg(long)]
    revision: u32,
    #[command(flatten)]
    note: NoteArgs,
}

#[derive(Args)]
pub struct NoteArgs {
    #[arg(long, conflicts_with = "text_file")]
    text: Option<String>,
    /// Read at most 4 KiB of UTF-8 from a regular file (not stdin)
    #[arg(long, conflicts_with = "text")]
    text_file: Option<PathBuf>,
    /// Retry identity; defaults to cli- plus 32 hex digits of the request digest
    #[arg(long)]
    request_key: Option<String>,
}
impl NoteArgs {
    fn text(&self) -> Result<String> {
        if let Some(text) = &self.text {
            return Ok(text.clone());
        }
        let Some(path) = &self.text_file else {
            return Ok(String::new());
        };
        require(
            std::fs::metadata(path)
                .with_context(|| format!("reading {} metadata", path.display()))?
                .is_file(),
            AnnotationCode::InvalidText,
            "note input must be a regular UTF-8 file",
        )?;
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        require(
            file.metadata()
                .context("reading note file metadata")?
                .is_file(),
            AnnotationCode::InvalidText,
            "note input must be a regular UTF-8 file",
        )?;
        let mut bytes = Vec::new();
        file.take((NOTE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading {}", path.display()))?;
        require(
            bytes.len() <= NOTE_BYTES,
            AnnotationCode::AnnotationLimit,
            "note exceeds 4 KiB",
        )?;
        String::from_utf8(bytes).map_err(|_| {
            annotation_error(
                AnnotationCode::InvalidText,
                "note file is not UTF-8",
                Value::Null,
            )
        })
    }
}

#[derive(Args)]
pub struct PageArgs {
    #[arg(long)]
    cursor: Option<String>,
    /// Items per page, 1..64 (default 16); encoded bytes also limit each page
    #[arg(long)]
    limit: Option<u32>,
}
impl PageArgs {
    fn query(self) -> AnnotationsQuery {
        AnnotationsQuery {
            cursor: self.cursor,
            limit: self.limit,
            ..Default::default()
        }
    }
}

impl AnnotationCmd {
    pub fn mutates(&self) -> bool {
        match self {
            Self::Open { .. } | Self::Reply(_) | Self::Resolve(_) | Self::Reopen(_) => true,
            Self::List { .. } | Self::Show { .. } => false,
        }
    }
    /// Supplemental agent-shell guard. PM/assistant/run attribution remains Entry's job;
    /// these two generic harness markers are not inputs to canonical event attribution.
    pub fn preflight(&self) -> Result<()> {
        require(
            !self.mutates()
                || ["AI_AGENT", "PI_CODING_AGENT"]
                    .iter()
                    .all(|key| std::env::var_os(key).is_none()),
            AnnotationCode::NotOwner,
            "agent shells cannot author owner annotations",
        )
    }
}

fn range(value: Option<String>) -> Result<Option<LineRange>> {
    value
        .map(|value| {
            let parsed = value.split_once('-').and_then(|(start, end)| {
                Some(LineRange {
                    start: start.parse().ok()?,
                    end: end.parse().ok()?,
                })
            });
            let range = parsed.ok_or_else(|| {
                annotation_error(
                    AnnotationCode::InvalidRange,
                    "expected lines A-B",
                    json!(value),
                )
            })?;
            range.validate()?;
            Ok(range)
        })
        .transpose()
}

fn mutate(
    store: &Store,
    task: &str,
    note: &NoteArgs,
    action: AnnotationAction,
) -> Result<AnnotationResult> {
    let task = Store::normalize_id(task)?;
    let digest = payload_sha256(&task, &action)?;
    let key = note
        .request_key
        .clone()
        .unwrap_or_else(|| format!("cli-{}", &digest[..32]));
    let cwd = std::env::current_dir().context("reading annotation caller cwd")?;
    annotations::mutate(store, &AnnotationActor::owner_cli(cwd), &task, &key, action)
}

pub fn run(start: &Path, entry: &crate::entry::Entry, args: AnnotationArgs) -> Result<()> {
    args.cmd.preflight()?;
    // No observation/config event writes on reads, replays or refused mutations.
    let store = Store::open_unobserved(&Store::find_root(start)?)?;
    entry.apply(&store)?;
    if args.cmd.mutates() {
        // Canonical attribution rejects PMs/run callers before reading note files.
        // Service admission repeats under the task lease, including before replay.
        store.annotation_owner_admission()?;
    }
    let result = match args.cmd {
        AnnotationCmd::List {
            task,
            state,
            anchor,
            page,
        } => {
            let mut query = page.query();
            query.state = state.map(|state| {
                if state == "open" {
                    AnnotationState::Open
                } else {
                    AnnotationState::Resolved
                }
            });
            query.anchor = anchor;
            return print_view(&annotations::list(&store, &task, &query)?, args.json);
        }
        AnnotationCmd::Show { task, thread, page } => {
            return print_view(
                &annotations::show(&store, &task, &thread, &page.query())?,
                args.json,
            );
        }
        AnnotationCmd::Open {
            task,
            run,
            base,
            head,
            path,
            old,
            lines,
            note,
        } => {
            let action = AnnotationAction::Open(OpenAnnotation {
                expected_run: run,
                base,
                head,
                path,
                side: if old {
                    AnnotationSide::Old
                } else {
                    AnnotationSide::New
                },
                range: range(lines)?,
                text: note.text()?,
            });
            mutate(&store, &task, &note, action)?
        }
        AnnotationCmd::Reply(change) => change_thread(&store, change, AnnotationAction::Reply)?,
        AnnotationCmd::Resolve(change) => change_thread(&store, change, AnnotationAction::Resolve)?,
        AnnotationCmd::Reopen(change) => change_thread(&store, change, AnnotationAction::Reopen)?,
    };
    if args.json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!(
            "{} {} revision {} {:?}{}",
            result.task,
            result.thread,
            result.revision,
            result.state,
            if result.replayed { " (replayed)" } else { "" }
        );
    }
    Ok(())
}

fn change_thread(
    store: &Store,
    change: ChangeArgs,
    action: fn(ChangeAnnotation) -> AnnotationAction,
) -> Result<AnnotationResult> {
    let payload = ChangeAnnotation {
        thread: change.thread,
        expected_revision: change.revision,
        text: change.note.text()?,
    };
    mutate(store, &change.task, &change.note, action(payload))
}

fn print_view(view: &AnnotationsView, json: bool) -> Result<()> {
    // JSON is intentionally compact: pretty printing could exceed the service's encoded budget.
    if json {
        println!("{}", serde_json::to_string(view)?);
    } else {
        println!("{}", serde_json::to_string_pretty(view)?);
    }
    Ok(())
}

pub fn error_json(error: &anyhow::Error) -> Value {
    if let Some(error) = error.downcast_ref::<AnnotationError>() {
        json!({"error":error})
    } else if error.is::<crate::entry::AssistantRefused>() {
        json!({"error":{"code":"not_owner","message":error.to_string(),"detail":null}})
    } else {
        json!({"error":{"code":"internal_error","message":format!("{error:#}"),"detail":null}})
    }
}
