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

impl FrameSources {
    pub fn load(ctx: &Ctx) -> Result<Self> {
        let board = ctx.store.board(None)?;
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
                shell.artifacts = head.artifacts_of(&run.id)?;
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
