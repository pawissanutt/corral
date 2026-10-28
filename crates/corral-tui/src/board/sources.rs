//! Bounded inputs of a board frame. The caller supplies the frame's pinned store snapshot.
use super::*;
use corral_core::{
    RunObligations, TaskHead,
    backend::{HeadQuery, StatusCount},
};

pub(super) struct FrameSources {
    pub tasks: Vec<Task>,
    pub heads: Vec<TaskHead>,
    pub obligations: HashMap<String, RunObligations>,
    pub status_counts: Vec<StatusCount>,
    pub slot_usage: SlotUsage,
    pub done_ids: HashSet<String>,
    pub pins: Vec<shelf::Entry>,
}

/// The previous frame's open board per store, kept without its read view.
static LAST_BOARD: std::sync::Mutex<Option<(std::path::PathBuf, corral_core::Board)>> =
    std::sync::Mutex::new(None);

/// The open board, reading only the tasks changed since the previous frame of this store.
fn open_board(ctx: &Ctx) -> Result<corral_core::Board> {
    let root = ctx.store.root().to_path_buf();
    let mut last = LAST_BOARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let board = match last.take() {
        Some((previous_root, previous)) if previous_root == root => {
            ctx.store.board_since(&previous)?
        }
        _ => ctx.store.board(None)?,
    };
    *last = Some((root, board.clone().detach()));
    Ok(board)
}

impl FrameSources {
    pub fn load(ctx: &Ctx) -> Result<Self> {
        let board = open_board(ctx)?;
        let status_counts = board.status_counts;
        let slot_usage = SlotUsage::from_heads(&board.heads, &ctx.store.config.limits);
        let mut heads = board.heads;
        // The configured Done viewport bounds the page independently of terminal history.
        heads.extend(ctx.store.heads_page(
            &HeadQuery::RecentByStatus {
                status: "done".into(),
            },
            u32::try_from(ctx.store.config.ui.done_max).unwrap_or(u32::MAX),
        )?);
        let pins = shelf::load_heads(ctx)?;
        let pin_ids = pins
            .iter()
            .filter_map(|entry| PinRef::parse(&entry.pin.target).ok())
            .map(|pin| pin.task().to_owned())
            .filter(|id| !heads.iter().any(|h| h.id == *id))
            .collect();
        heads.extend(ctx.store.heads(&HeadQuery::Ids(pin_ids))?);
        let relation_ids = heads
            .iter()
            .flat_map(|h| h.depends_on.iter().chain(&h.children))
            .cloned()
            .collect();
        let done_ids = ctx
            .store
            .heads(&HeadQuery::Ids(relation_ids))?
            .into_iter()
            .filter(|h| h.status == Status::Done)
            .map(|h| h.id.clone())
            .collect();
        heads.sort_by_key(|head| head.num);
        let mut obligations = HashMap::new();
        let mut tasks = Vec::new();
        for head in &heads {
            let mut shell = head.to_shell_task()?;
            if let Some(run) = &head.current_run {
                shell
                    .artifacts
                    .extend(head.latest_artifact_kind(&run.id, corral_core::ArtifactKind::Diff)?);
                obligations.insert(head.id.clone(), head.card_obligations(&run.id)?);
            }
            tasks.push(shell);
        }
        Ok(Self {
            tasks,
            heads,
            obligations,
            status_counts,
            slot_usage,
            done_ids,
            pins,
        })
    }
}
