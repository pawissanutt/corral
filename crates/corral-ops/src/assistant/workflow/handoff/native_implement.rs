//! Native "Implement here": the owner's current-revision choice that ends planning and opens
//! scratch work for a ready plan's declared scope. Bound like every native choice; the plan
//! artifact alone never grants execution, and this is never shipping consent.
use anyhow::{Context as _, Result, ensure};
use corral_core::assistant::AssistantContainer;
use corral_core::assistant::AssistantScratch;
use corral_core::assistant_workflow::{
    DesignStage, ExecutionScope, ScopeOwner, SourceAssumptions, Workflow, WorkflowPhase,
};
use corral_core::{Store, blob_ref::BlobHash};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::publication::{artifact, load_artifact};
use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImplementChoice {
    pub project: String,
    /// The exact ready plan revision the owner saw; a newer revision refuses.
    pub plan_revision: u32,
}

fn ready(store: &Store, w: &Workflow) -> Result<Option<(u32, ExecutionScope)>> {
    let Some(plan) = w.current_plan.as_ref() else {
        return Ok(None);
    };
    let Some(scope) = plan.execution_scope.as_ref() else {
        return Ok(None);
    };
    if w.owner != ScopeOwner::Local
        || w.phase != WorkflowPhase::Planning
        || plan.stage != DesignStage::Ready
    {
        return Ok(None);
    }
    let scope: ExecutionScope = serde_json::from_slice(&load_artifact(store, scope)?)
        .context("implement: unreadable declared execution scope")?;
    Ok(Some((plan.revision, scope)))
}

/// What the host may offer: the ready plan revision and its declared scope, else `Null`.
pub fn offer(store: &Store, container: &AssistantContainer) -> Result<Value> {
    let Some(index) = &container.workflow else {
        return Ok(Value::Null);
    };
    Ok(match ready(store, &index.current)? {
        Some((revision, scope)) => match source_holds(store, &index.current) {
            Ok(_) => json!({"project": scope.project, "plan_revision": revision,
                "paths": scope.paths, "actions": scope.actions}),
            Err(e) => json!({"unavailable": format!("{e:#}")}),
        },
        None => Value::Null,
    })
}

/// Refuse a choice that names another revision or project before it is consumed.
pub(super) fn precheck(ctx: &Ctx, session: &str, choice: &ImplementChoice) -> Result<()> {
    let c = ctx.store.get_assistant_container(session)?;
    let w = super::super::current(&c)?;
    let (revision, scope) = ready(&ctx.store, w)?.context(
        "implement_unavailable: only a ready local plan with a declared scope, while planning",
    )?;
    ensure!(
        revision == choice.plan_revision && scope.project == choice.project,
        super::native_view::STALE
    );
    source_holds(&ctx.store, w)?;
    Ok(())
}

/// Revalidate the ready plan's recorded source observations against the registered project:
/// HEAD, base ancestry and every observed file's digest or absence. Files are read no-follow,
/// regular-only and capped; a parent that escapes the project refuses. Returns the source HEAD.
fn source_holds(store: &Store, w: &Workflow) -> Result<String> {
    const STALE_SOURCE: &str =
        "implement_stale_source: the plan's observed source changed; revise the plan first";
    let plan = w
        .current_plan
        .as_ref()
        .context("implement: no current plan")?;
    let reference = plan
        .assumptions
        .as_ref()
        .context("implement_unavailable: ready plan without source assumptions")?;
    let a: SourceAssumptions = serde_json::from_slice(&load_artifact(store, reference)?)
        .context("implement: unreadable source assumptions")?;
    let repo = store
        .project_path(&a.project)?
        .canonicalize()
        .context("implement: resolve the plan's project")?;
    let head = String::from_utf8(super::git::git(
        &repo,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        None,
    )?)?;
    ensure!(head.trim() == a.source_head, STALE_SOURCE);
    super::git::git(
        &repo,
        &["merge-base", "--is-ancestor", &a.base_head, &a.source_head],
        None,
    )
    .context(STALE_SOURCE)?;
    for file in &a.files {
        let path = repo.join(&file.path);
        if let Some(parent) = path.parent().filter(|p| p.exists()) {
            ensure!(
                parent.canonicalize()?.starts_with(&repo),
                "implement_source_unsupported: observed path escapes the project"
            );
        }
        let actual = match path.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            _ => Some(BlobHash::of(
                &super::content::regular(&path, super::content::FILE_MAX)
                    .context("implement_source_unsupported")?
                    .0,
            )),
        };
        ensure!(actual == file.sha256, STALE_SOURCE);
    }
    Ok(a.source_head)
}

/// The typed execution grant one consumed Implement here choice records. It names its scratch
/// identity before the scratch exists, so an interruption reconciles to exactly that scratch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub version: u32,
    pub plan_revision: u32,
    pub scope: ExecutionScope,
    pub scratch: String,
    pub base: String,
    pub evidence: BlobHash,
    pub ships: bool,
}

/// The current grant, valid only against the current ready plan revision and its scope.
pub fn grant(store: &Store, w: &Workflow) -> Result<Grant> {
    let reference = w
        .execution
        .as_ref()
        .context("implement: no execution grant")?;
    let g: Grant = serde_json::from_slice(&load_artifact(store, reference)?)
        .context("implement_grant_invalid: unreadable execution grant")?;
    let plan = w
        .current_plan
        .as_ref()
        .context("implement: no current plan")?;
    let scope: ExecutionScope = serde_json::from_slice(&load_artifact(
        store,
        plan.execution_scope
            .as_ref()
            .context("implement: no declared scope")?,
    )?)?;
    ensure!(
        g.version == 1
            && !g.ships
            && plan.stage == DesignStage::Ready
            && g.plan_revision == plan.revision
            && g.scope == scope,
        "implement_grant_invalid: execution grant does not match the current ready plan"
    );
    Ok(g)
}

/// Local scoped authority: an executing local workflow whose valid grant's scratch is recorded.
pub fn granted(store: &Store, session: &str, container: &AssistantContainer) -> Result<bool> {
    let Some(index) = &container.workflow else {
        return Ok(false);
    };
    let w = &index.current;
    if w.owner != ScopeOwner::Local || w.phase != WorkflowPhase::Executing {
        return Ok(false);
    }
    let g = grant(store, w)?;
    Ok(store
        .get_assistant(session)?
        .scratch
        .iter()
        .any(|s| s.slug == g.scratch && s.project == g.scope.project && s.base == g.base))
}

/// Finish an interrupted Implement here: create exactly the granted scratch, or return it.
pub fn reconcile(ctx: &Ctx, session: &str) -> Result<Option<AssistantScratch>> {
    let c = ctx.store.get_assistant_container(session)?;
    let Some(index) = &c.workflow else {
        return Ok(None);
    };
    let w = &index.current;
    if w.owner != ScopeOwner::Local || w.phase != WorkflowPhase::Executing {
        return Ok(None);
    }
    let g = grant(&ctx.store, w)?;
    let record = ctx.store.get_assistant(session)?;
    if let Some(s) = record.scratch.iter().find(|s| s.slug == g.scratch) {
        ensure!(
            s.project == g.scope.project && s.base == g.base,
            "implement_grant_invalid: scratch {} is not the granted source",
            g.scratch
        );
        return Ok(Some(s.clone()));
    }
    let scratch = crate::assistant::scratch::create(
        ctx,
        session,
        &g.scope.project,
        Some(&g.scratch),
        Some(&g.base),
    )
    .context("implement_recovery: the granted scratch could not be created")?;
    Ok(Some(scratch))
}

pub(super) fn run(
    ctx: &Ctx,
    session: &str,
    choice: &ImplementChoice,
    shown: &str,
    evidence: &BlobHash,
) -> Result<Value> {
    let c = ctx.store.get_assistant_container(session)?;
    ensure!(
        super::native_view::view(ctx, session, &c)?.digest == shown,
        super::native_view::STALE
    );
    let w = super::super::current(&c)?.clone();
    let (revision, scope) = ready(&ctx.store, &w)?.context(
        "implement_unavailable: only a ready local plan with a declared scope, while planning",
    )?;
    ensure!(
        revision == choice.plan_revision && scope.project == choice.project,
        super::native_view::STALE
    );
    ensure!(
        super::super::boundary::activities(&ctx.store, session)?.is_empty(),
        "handoff_busy: source effects must settle before implementation"
    );
    let base = source_holds(&ctx.store, &w)?;
    // The grant names its scratch before it exists; reconcile creates exactly that one.
    let slug = format!("impl-{}", &evidence.as_str()[..8]);
    ensure!(
        !ctx.store
            .get_assistant(session)?
            .scratch
            .iter()
            .any(|s| s.slug == slug),
        "implement_grant_invalid: granted scratch identity already in use"
    );
    let g = Grant {
        version: 1,
        plan_revision: revision,
        scope,
        scratch: slug,
        base,
        evidence: evidence.clone(),
        ships: false,
    };
    ctx.store.settle_assistant_container(session, |c, _| {
        let index = c.workflow.as_mut().context("workflow missing")?;
        let now = &index.current;
        ensure!(
            now.id == w.id
                && now.state_revision == w.state_revision
                && now.authority_epoch == w.authority_epoch,
            super::native_view::STALE
        );
        let execution = artifact(&ctx.store, session, &w.id, &serde_json::to_vec(&g)?)?;
        index.authority_epoch = index.authority_epoch.checked_add(1).context("epoch")?;
        let now = &mut index.current;
        now.authority_epoch = index.authority_epoch;
        now.state_revision = now.state_revision.checked_add(1).context("revision")?;
        now.phase = WorkflowPhase::Executing;
        now.execution = Some(execution);
        Ok(())
    })?;
    crate::agent::failpoint("native_implement_granted");
    let scratch = reconcile(ctx, session)?.context("implement: grant missing after settle")?;
    Ok(
        json!({"action":"implement","plan_revision":revision,"scratch":scratch.slug,
        "path":scratch.path,"ships":false}),
    )
}
