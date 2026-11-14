// Corral app mockup data.
// REAL: board snapshot from ~/corral-board-opus/data.js (corral list/show, 2026-11-07 ~22:12 UTC);
//       T-868 task body, question T-868/R1/Q1, artifacts, comments, gate history and Pi patch hunk
//       (corral show T-868 --json and packages/corral-pi/patches, read 2026-11-11).
// ILLUSTRATIVE (marked `illus: true` or listed in DESIGN.md): 18:40 last-visit marker, terminal tails,
//       transcripts, thread replies, cost estimates on options, quota/usage numbers, machines, devices,
//       pairing codes, the 64-member scale plan, the review diff's surrounding context lines.
window.D = (() => {
  const NOW = "22:12", LAST_VISIT = "18:40";

  // rail = run, gate, review, land: done · active · fail · stale · next · na
  const TASKS = {
    "T-868": { title: "host-queue-ledger", plan: "T-870", actor: "you", rail: ["active", "next", "next", "next"], now: "needs you · restore policy", age: "3h", cost: null, changed: true,
      agent: "pi-sol xhigh", run: "R1", started: "18:58", machine: "workstation",
      goal: "Give queued Pi-native input stable item identities and a durable lifecycle that keeps delivery acknowledgement (admitted, observed) distinct from execution (queued, started, then turn completion), and let authorized callers inspect, withdraw and edit queued items with defined races against consumption.",
      needs: [["T-701", "done"], ["T-867", "stale"]], unblocks: [], steps: [["done", "Verify Pi 1.5.0 metadata, refresh frozen deps"], ["done", "Stable queue identities + lifecycle journal"], ["active", "Restore policy on stock/plain paths"], ["next", "Bridge queue_state / queue_withdraw"], ["next", "Rebase, quick gate, attach proof"]],
      next: ["You", "answer T-868/R1/Q1"], pane: "w12C:p2" },
    "T-890": { title: "pm-bounded-read-contract", plan: "T-893", actor: "corral", rail: ["done", "active", "next", "next"], now: "quick gate 1751bd2", age: "9m", cost: 3.90, changed: true,
      agent: "pi-sol xhigh", run: "R1", started: "19:31", machine: "workstation",
      steps: [["done", "Read plan, inspect scope/read contracts"], ["done", "Shared bounded projection + CLI/native adapters"], ["done", "Verify live paging, byte caps, compatibility"], ["active", "Commit, rebase, gate and attach proof"]],
      next: ["Corral", "quick gate finishes → independent review"], needs: [["T-885", "done"]], unblocks: ["T-891"], pane: "w12C:p4",
      tail: ["$ scripts/gate.sh --tier quick", "  build     corral-ops corral-tui   41.2s", "  nextest   corral-ops  412/980", "    PASS  pm_workset::bounded_page", "    PASS  pm_workset::byte_cap_trims", "    PASS  pm_next::keyset_live_update", "    ....  read_view::scope_paging"] },
    "T-894": { title: "api-connections", plan: "T-898", actor: "corral", rail: ["done", "active", "next", "next"], now: "gate 6b36916", age: "6m", cost: 18.11, changed: true,
      agent: "pi-sol xhigh", run: "R1", started: "19:33", machine: "workstation",
      steps: [["done", "Trace binding, auth, guarded transport"], ["done", "Explicit isolated connections"], ["done", "Focused offline regression checks"], ["active", "Commit, rebase, gate and attach evidence"]],
      next: ["Corral", "gate finishes → independent review"], needs: [["T-888", "done"]], unblocks: ["T-895", "T-896"], pane: "gate",
      tail: ["$ scripts/gate.sh", "  Compiling corral-ops", "  Compiling corral-pi-host", "  Compiling corral-tui"] },
    "T-867": { title: "queue-contract-core", plan: "T-870", actor: "corral", rail: ["done", "stale", "next", "next"], now: "stale 5949a4b · re-gate", age: "7m", cost: 1.83, changed: true,
      agent: "pi-sol xhigh", run: "R1", started: "20:40", machine: "workstation",
      steps: [["done", "Catalog defaults, negotiation, immutable history"], ["done", "Monotonic execution records, pending indexes"], ["done", "Immutable intents/outcomes, atomic edits"], ["active", "Targeted checks, evidence, rebase, quick gate"]],
      next: ["Corral", "re-gate on the rebased candidate"], needs: [["T-701", "done"]], unblocks: ["T-868", "T-869"], pane: "worker",
      tail: ["worker idle · candidate 5949a4b", "gate result: stale (base moved)"] },
    "T-852": { title: "Co-work C1: shared completion engine + failure triage", plan: "T-851", actor: "held", rail: ["done", "stale", "next", "next"], now: "held · external op", age: "2h44", cost: 43.40, changed: true,
      agent: "pi-sol xhigh", run: "R1", started: "09:43", machine: "workstation", round: 2,
      steps: [["done", "Trace design and existing pipeline"], ["done", "Durable async submit, current-driver"], ["done", "Focused regression checks"], ["done", "Commit, rebase, receipted quick gate"], ["done", "Reproduce + fix infra episode (round 2)"], ["done", "Validate, rebase, fresh quick gate"]],
      next: ["Corral", "external operation must finish (2h44m), then re-gate"], needs: [["T-802", "done"]], unblocks: ["T-851"], pane: "worker",
      tail: ["worker done · round 2", "gate ◌ stale af7a6dc"] },
    "T-871": { title: "annotation-service", plan: "T-874", actor: "worker", rail: ["active", "next", "next", "next"], now: "working · step 1/3", age: "55m", cost: 3.29, changed: true,
      agent: "pi-sol xhigh", run: "R1", started: "21:17", machine: "workstation",
      steps: [["active", "Versioned records + canonical indexed persistence"], ["next", "Git anchors, authorized mutations, bounded reads"], ["next", "Targeted acceptance tests and quick gate"]],
      next: ["Worker", "finish step 1 of 3"], needs: [["T-699", "done"]], unblocks: ["T-872", "T-873"], pane: "w14A:p1",
      tail: ["Edit  crates/corral-core/src/annot/record.rs", "      +118 -4", "Bash  cargo check -p corral-core", "      ok 14.8s"] },
    "T-854": { title: "Co-work C3: save, take over, take back", plan: "T-851", actor: "worker", rail: ["active", "next", "next", "next"], now: "working · publication", age: "1h", cost: 7.56, changed: true,
      agent: "pi-sol xhigh", run: "R1", started: "01:10", machine: "workstation",
      steps: [["done", "Read design, inspect ownership journal and admission seams"], ["active", "Held task publication, journal recovery, scoped intent"], ["next", "Stage-aware adoption and plan-only stop guards"], ["next", "Settled take-back and landing serialization"], ["next", "Targeted lifecycle regressions and criterion evidence"], ["next", "Commit, rebase, receipted quick gate"]],
      next: ["Worker", "finish the publication step"], needs: [["T-852", "done"], ["T-853", "done"]], unblocks: ["T-855"], pane: "w15B:p1",
      tail: ["Read  artifacts/T-854/R1/edf0576e-c3-checkpoint-notes.md", "Edit  crates/corral-ops/src/cowork/publish.rs", "      +64 -12", "Bash  cargo test -p corral-ops cowork::publish", "      running 9 tests"] },
    "T-891": { title: "pm-obligation-read-view", plan: "T-893", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-890", needs: [["T-885", "done"], ["T-890", "active"]], unblocks: ["T-892"] },
    "T-892": { title: "pm-read-costs-and-guidance", plan: "T-893", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-891", needs: [["T-891", "next"]], unblocks: [] },
    "T-895": { title: "failure-signals", plan: "T-898", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-894", needs: [["T-894", "active"]], unblocks: [] },
    "T-896": { title: "metered-fallback", plan: "T-898", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-894", needs: [["T-894", "active"]], unblocks: ["T-897"] },
    "T-897": { title: "role-continuations", plan: "T-898", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-896", needs: [["T-896", "next"]], unblocks: [] },
    "T-869": { title: "inbox-queue-ops", plan: "T-870", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-867", needs: [["T-867", "stale"]], unblocks: [] },
    "T-872": { title: "annotation-owner-cli", plan: "T-874", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-871", needs: [["T-871", "active"]], unblocks: [] },
    "T-873": { title: "annotation-pi-host-api", plan: "T-874", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-871", needs: [["T-871", "active"]], unblocks: [] },
    "T-855": { title: "Co-work C4: assistant + board handoff TUI", plan: "T-851", actor: "queued", rail: ["next", "next", "next", "next"], now: "after T-854" },
    "T-883": { title: "Record reviewer child outcome for co-work resubmit", actor: "queued", rail: ["next", "next", "next", "next"], now: "queued · capacity" },
    "T-832": { title: "Co-work finish missing usage after diag failure", actor: "queued", rail: ["next", "next", "next", "next"], now: "queued · capacity" },
    "T-701": { title: "Pi input queue lifecycle design", actor: "done", rail: ["na", "na", "na", "na"], now: "plan P2 approved" },
    "T-885": { title: "PM supervisor evolution: bounded reads", actor: "done", rail: ["na", "na", "na", "na"], now: "plan P2 accepted" },
  };

  const GROUPS = [
    { key: "you", label: "Needs you", ids: ["T-868"], extra: [{ kind: "plan", id: "T-885", title: "plan P2 · accept?", note: "reviewed · 3 members" }, { kind: "ask", id: "PM/Q7", title: "T-852 repair budget: re-gate only, or one more round?", note: "12m · sample", illus: true }] },
    { key: "corral", label: "Corral is verifying", ids: ["T-890", "T-894", "T-867"] },
    { key: "worker", label: "Workers producing", ids: ["T-871", "T-854"] },
    { key: "queued", label: "Queued", ids: ["T-891", "T-892", "T-895", "T-896", "T-897", "T-869", "T-872", "T-873"], more: 15 },
    { key: "held", label: "Held", ids: ["T-852"] },
    { key: "parked", label: "Parked", ids: [], parked: { n: 32, split: "25 corral · 5 corral-ext-mitigraph · 2 mitimind-lite", note: "Each has a dispatch hold you set. They wait here, not under Needs you." } },
  ];

  const HOLDS = [["T-130", "mitimind-lite", "Knowledge engine 31: verify owner-approved merged delivery", "11-05", "You answered 11-07 08:12: keep held; finish Corral first"], ["T-157", "corral", "PRF-05: first-run and profile-selection TUI flows", "11-05"], ["T-158", "corral", "PRF-06: mapping refresh and explicit legacy migration", "11-05"], ["T-187", "corral", "Cost and performance analytics", "10-20"], ["T-194", "corral", "Board glance: spend summary and report shortcut", "11-05"], ["T-209", "corral", "Agent inbox 4/5: omp corral-inbox extension adapter", "11-05"]];

  // T-868/R1/Q1: question text, options, recommendation and refs are real (corral question show, 2026-11-08).
  const Q = {
    key: "T-868/R1/Q1", rev: 2, kind: "owner only", blocking: true, task: "T-868", plan: "T-870",
    from: "worker T-868 R1", via: "escalated by the PM", asked: "19:12", age: "3h",
    short: "Restore policy: pane-origin input only, or all origins?",
    text: "T-701@P2 leaves one restore-policy decision open for T-868: on stock/plain paths where Esc/dequeue restoration is enabled, should restoration remove only pane-origin queued input, or restore all origins? Rich mode will remain read-only/count-only with no restoration either way.",
    why: "Restore only pane-origin input, so Corral/extension instructions cannot become an owner draft. I can implement the ledger and other controls independently while awaiting this decision.",
    refs: ["T-701@P2", "packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.5.0.patch"],
    options: [
      { label: "Restore only pane-origin input; leave Corral/extension items queued", rec: true,
        next: "Worker adds the restoreInput hook: pane items return their original text, others stay queued. Then rebase and quick gate.",
        unblocks: "T-868 step 3/5 resumes now; T-870 member 2 of 3 can reach its gate.",
        costs: "≈1 step on pi-sol xhigh (≈$2–4, est.) · Esc on plain paths no longer hands Corral/extension items to your draft.",
        proof: "A · diff hunk" },
      { label: "Retain restore-all; journal removals and treat resubmission as new pane input",
        next: "Worker journals every removal and re-admits restored text as new pane input with fresh identity.",
        unblocks: "Same unblock, later: adds a journal record type and its replay tests.",
        costs: "≈2 steps (≈$6–9, est.) · Corral/extension instructions can land in your draft and be resent as yours.",
        proof: "A · file excerpt" },
    ],
    waits: [["T-868", "Blocked until you answer", "you"], ["T-870", "plan member 2 of 3 · final accept needs it", "plan"], ["T-869", "independent · after T-867, not this answer", "info"]],
    history: [
      { at: "11-07 · T-701@P2", who: "you", text: "Approved plan P2: keep main's rich readOnlyQueue; leave the restore-policy choice open only where restoration is enabled." },
      { at: "11-07 · T-701@P2", who: "you", text: "Asked for a Pi 1.5.0 rebase, not implementation admission." },
      { at: "R1 · checkpoint 2", who: "worker", text: "Current queue implementation passes typecheck and five deterministic host/control tests (1.64 s)." },
      { at: "R1 · report", who: "worker", text: "Pre-gate queue acceptance evidence and reconciled helper findings; owner restore policy remains pending." },
      { at: "R1 · log", who: "worker", text: "Native Alt+Up and Esc restore only pane text at 52x24 and 140x46; external identities and draft preserved." },
    ],
    thread: [
      { who: "you", via: "app:pawis-iphone", at: "21:58", text: "Does option 1 change anything in rich mode?", illus: true },
      { who: "worker T-868", at: "21:59", text: "No. Rich mode stays read-only/count-only; neither option restores there.", illus: true },
    ],
    answeredElsewhere: { at: "22:14", where: "PM pane", via: "pm-pane", option: 0 },
  };

  // Real Pi 1.5.0 patch hunk (AgentSession.clearQueue), as on main.
  const HUNK = {
    file: "packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.5.0.patch", at: "@@ -1959,13 +1990,16 @@ class AgentSession",
    lines: [
      [" ", "     * @returns Object with steering and followUp arrays"],
      [" ", "     */"],
      ["-", "    clearQueue() {"],
      ["-", "        const steering = [...this._steeringMessages];"],
      ["-", "        const followUp = [...this._followUpMessages];"],
      ["-", "        this.agent.clearAllQueues();"],
      ["+", "    clearQueue(options) {"],
      ["+", "        const steering = [], followUp = [];"],
      ["+", "        for (const message of this.agent.getQueuedMessages()) {"],
      ["+", "            const tag = this._queueTags.get(message);"],
      ["+", "            const restored = options?.restore && this._queueObserver?.restoreInput"],
      ["+", "                ? this._queueObserver.restoreInput(tag) : undefined;"],
      ["+", "            if (options?.restore && restored === undefined) continue;"],
      ["+", "            this.agent.removeQueuedMessage(message, options?.restore ? \"pane_restore\" : \"host\");"],
      ["+", "        }"],
      [" ", "        return { steering, followUp };"],
    ],
  };
  const EXCERPT = {
    file: "HostQueueObserver (same patch, lines 18–24)",
    lines: ["export interface HostQueueObserver {", "    input(input: { text; images?; source: InputSource; mode }): unknown;", "    lifecycle(event: { tag; event: PromptDisposition | \"rejected\" }): void;", "    queue(change: AgentQueueChange & { entries }): void;", "    /** Original pane text when native restoration is allowed;", "        undefined keeps the item queued. */", "    restoreInput?(tag: unknown): string | undefined;", "}"],
  };
  const LOGTAIL = { file: "artifacts/T-868/R1/9a87439d-plain-restore-check.log", illus: true,
    lines: ["plain-restore 52x24   Alt+Up  restored 1 pane item   kept 2 external", "plain-restore 52x24   Esc     restored 1 pane item   kept 2 external", "plain-restore 140x46  Alt+Up  restored 1 pane item   kept 2 external", "plain-restore 140x46  Esc     draft preserved        kept 2 external", "external identities unchanged: q-g13-5c1e…-0007, q-g13-5c1e…-0008", "PASS  4/4"] };

  const PLANS = [
    { id: "T-885", rev: "P2", state: "awaiting", title: "PM supervisor evolution: bounded reads, warm-context episodes", delivery: "main",
      problem: "The PM owner and agent need a bounded and consistent view of current responsibilities; operational continuity should not depend on an ever-growing conversation.",
      goal: "Deliver a bounded first wave of PM information and lifecycle visibility improvements, while reviewing a longer-term hybrid supervisor design without authorizing it wholesale.",
      changes: ["A bounded common semantic read contract for PM CLI and native tools, with scope-specific rendering and complete detail access.", "Existing attention, inbox, question, wait and receipt records as one read-only obligation view; no second scheduler or task database.", "Measure the improved loop and document an evidence-gated warm-context episode pilot for later review."],
      retained: ["One logical PM, current conversation lifetime, existing sync and wake ownership, current deterministic executors.", "Current scope checks, owner authority, receipts, candidate identities, gates, independent review and landing checks."],
      nonGoals: ["No fresh model session per event or mandatory conversation rollover.", "No new daemon, scheduler, obligation database, memory engine or manager hierarchy.", "No provider/model change, paid evaluation, recurring model job or background reflection."],
      members: [["pm-bounded-read-contract", []], ["pm-obligation-read-view", ["pm-bounded-read-contract"]], ["pm-read-costs-and-guidance", ["pm-obligation-read-view"]]],
      scenarios: 6, artifact: "artifacts/T-885/R0/b494fef5-pm-supervisor-plan-v2.md · 43.9 KB",
      reviews: [["P1", "fail", "changes", "pi-sol", "10:33", "Direction sound; two read-contract decisions (F1 pagination/fairness, F2) unresolved."], ["P2", "done", "approve", "pi-sol", "12:10", "F1 and F2 addressed. No blocking defect. Approval is for owner consideration only."]] },
    { id: "T-870", state: "executing", title: "Pi input: stable queue item lifecycle, editing, cancellation", members: ["T-867", "T-868", "T-869"], scenarios: 15, mapped: 0, accepted: "earlier",
      layers: [["T-867"], ["T-868", "T-869"]], blockers: ["T-867 required work is Running", "T-868 waits on your answer", "T-869 required work is Ready"] },
    { id: "T-893", state: "executing", title: "PM supervisor evolution: bounded reads, warm-context episodes", members: ["T-890", "T-891", "T-892"], scenarios: 6, mapped: 0, accepted: "you 19:12",
      layers: [["T-890"], ["T-891"], ["T-892"]], blockers: ["T-890 required work is Running", "T-891 required work is Ready", "T-892 required work is Ready"] },
    { id: "T-898", state: "executing", title: "Anthropic + OpenAI API connections, bounded subscription fallback", members: ["T-894", "T-895", "T-896", "T-897"], scenarios: 7, mapped: 0, accepted: "earlier",
      layers: [["T-894"], ["T-895", "T-896"], ["T-897"]], blockers: ["T-894 required work is Running", "T-895 required work is Ready", "T-896 required work is Ready", "T-897 required work is Ready"] },
    { id: "T-874", state: "executing", title: "Pi review annotations: durable candidate-bound threads", members: ["T-871", "T-872", "T-873"], scenarios: 12, mapped: 0, accepted: "earlier",
      layers: [["T-871"], ["T-872", "T-873"]], blockers: ["T-871 required work is Running", "T-872 required work is Ready", "T-873 required work is Ready"] },
    { id: "T-385", rev: "P5", state: "approved", title: "Design: reliable task progress and PM attention across harnesses", note: "4 approvals · no tracker published" },
  ];

  // Synthetic 64-member tracker (scale test, clearly labelled).
  const SCALE = (() => {
    const names = ["schema", "codec", "index", "cursor", "pager", "ledger", "replay", "fence", "lease", "probe", "bridge", "relay", "sweep", "audit", "quota", "notice"];
    const layers = [[14, 0, 0], [12, 1, 1], [10, 2, 1], [11, 1, 0], [9, 0, 0], [8, 0, 0]]; // [size, active, needs-you]
    let n = 0; const out = [];
    layers.forEach(([size, act, you], li) => {
      const L = [];
      for (let i = 0; i < size; i++) {
        n++; const id = "S-" + (900 + n);
        let st = li === 0 ? "landed" : li === 1 && i >= 5 + act + you ? "landed" : "waiting";
        if (li === 1 && i < 5) st = "landed";
        if (li === 1 && i === 5) st = "active"; if (li === 1 && i === 6) st = "you";
        if (li === 2 && i === 0) st = "active"; if (li === 2 && i === 1) st = "active"; if (li === 2 && i === 2) st = "you";
        if (li === 3 && i === 0) st = "held";
        L.push({ id, name: names[(n * 7) % names.length] + "-" + ((n * 13) % 97), st });
      }
      out.push(L);
    });
    return out;
  })();

  const CHANGES = [
    ["22:06", "T-894", "active", "gate started on 6b36916"],
    ["22:05", "T-867", "stale", "gate went stale on 5949a4b (base moved)"],
    ["22:03", "T-890", "active", "quick gate started on 1751bd2"],
    ["22:03", "T-852", "stale", "gate went stale on af7a6dc"],
    ["21:59", "T-868", "msg", "worker replied in thread on T-868/R1/Q1"],
    ["21:17", "T-871", "start", "worker started · pi-sol xhigh"],
    ["21:11", "T-890", "dot", "checkpoint 3 captured"],
    ["20:40", "T-867", "start", "worker started · pi-sol xhigh"],
    ["20:32", "T-890", "dot", "checkpoint 2 captured"],
    ["19:33", "T-894", "start", "worker started · pi-sol xhigh"],
    ["19:31", "T-890", "start", "worker started · pi-sol xhigh"],
    ["19:12", "T-893", "done", "you accepted plan T-885 P2 → 3 members published"],
  ];

  // Agents now (machine names illustrative; all agents run on the daemon's machine today).
  const AGENTS = [
    { id: "PM", role: "PM", task: null, title: "supervisor · generation 14", profile: "pi-sol xhigh", runtime: "herdr-core", pane: "pm:p0", state: "live", doing: "waiting · next wake on T-894 gate", since: "21:40", tail: ["wake  sync tick · 3 changes", "read  corral pm next --bounded", "note  T-852 budget question drafted (PM/Q7)", "idle  next wake on gate result"] },
    { id: "T-871", role: "worker", task: "T-871", title: "annotation-service", profile: "pi-sol xhigh", runtime: "herdr-core", pane: "w14A:p1", state: "working", doing: "Edit crates/corral-core/src/annot/record.rs", since: "21:17" },
    { id: "T-854", role: "worker", task: "T-854", title: "Co-work C3: save, take over, take back", profile: "pi-sol xhigh", runtime: "Herdr TUI (legacy)", pane: "w15B:p1", state: "working", doing: "cargo test -p corral-ops cowork::publish", since: "01:10" },
    { id: "T-868", role: "worker", task: "T-868", title: "host-queue-ledger", profile: "pi-sol xhigh", runtime: "herdr-core", pane: "w12C:p2", state: "asking", doing: "waiting on you · T-868/R1/Q1 · ledger work continues", since: "19:12", tail: ["Edit  packages/corral-pi/src/host/queue.ts", "      +41 -6", "Ask   T-868/R1/Q1 restore policy", "Bash  bun test test/queue-host.test.ts", "      16 pass"] },
    { id: "gate-T-890", role: "gate", task: "T-890", title: "quick gate 1751bd2", profile: "scripts/gate.sh", runtime: "herdr-core", pane: "g:T-890", state: "gating", doing: "nextest corral-ops 412/980", since: "22:03" },
    { id: "gate-T-894", role: "gate", task: "T-894", title: "gate 6b36916", profile: "scripts/gate.sh", runtime: "herdr-core", pane: "g:T-894", state: "gating", doing: "Compiling corral-tui", since: "22:06" },
    { id: "T-867", role: "worker", task: "T-867", title: "queue-contract-core", profile: "pi-sol xhigh", runtime: "herdr-core", pane: "w12C:p1", state: "idle", doing: "idle · candidate 5949a4b awaits re-gate", since: "22:05" },
  ];

  // Usage (illustrative numbers; provider/connection names follow real usage records where they exist).
  const USAGE = {
    providers: [
      { id: "openai-subscription", label: "OpenAI · subscription", window: "5 h rolling", used: 0.78, resets: "1h 46m", weekly: 0.61, today: 61.2, week: 318.4, profiles: [["pi-sol xhigh", 0.69, 53.7], ["pi-terra high", 0.07, 6.4], ["assistant · quick thread A-7", 0.02, 1.1]], series: [0.12, 0.2, 0.31, 0.38, 0.42, 0.47, 0.55, 0.61, 0.66, 0.72, 0.75, 0.78] },
      { id: "anthropic-subscription", label: "Anthropic · subscription", window: "5 h rolling", used: 0.34, resets: "3h 02m", weekly: 0.48, today: 22.7, week: 141.0, profiles: [["omo opus", 0.29, 19.1], ["claude sonnet review", 0.05, 3.6]], series: [0.05, 0.08, 0.1, 0.18, 0.22, 0.25, 0.27, 0.29, 0.3, 0.31, 0.33, 0.34] },
      { id: "openai-api", label: "OpenAI · API (metered)", window: "allowance $40 / day", used: 0.21, resets: "01:48", weekly: null, today: 8.4, week: 29.9, profiles: [["fallback · pi-sol", 0.21, 8.4]], series: [0, 0, 0, 0, 0.04, 0.04, 0.09, 0.12, 0.15, 0.18, 0.2, 0.21] },
    ],
    hours: ["11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22"],
    week: [["Thu", 38, 21, 2], ["Fri", 52, 18, 4], ["Sat", 12, 9, 0], ["Sun", 44, 26, 5], ["Mon", 57, 30, 6], ["Tue", 54, 14, 4], ["Wed", 61, 23, 8]],
  };

  const DEVICES = [
    { name: "pawis-desktop", platform: "Linux · desktop app", seen: "now", reach: "direct · loopback", self: true },
    { name: "pawis-iphone", platform: "iOS 19 · app", seen: "14m ago", reach: "relay · e2e" },
    { name: "pawis-laptop", platform: "Firefox · web client", seen: "2d ago", reach: "direct · tailscale" },
    { name: "terminal", platform: "corral app --tui · terminal target", seen: "5h ago", reach: "direct · loopback" },
  ];

  // Timeline lanes 09:00–22:12 (minutes from 09:00). Segment kinds: run gate review land idle stall ask.
  const TL = {
    start: 9 * 60, end: 22 * 60 + 12, lastVisit: 18 * 60 + 40,
    lanes: [
      { id: "T-852", label: "Co-work C1", segs: [["run", "09:43", "12:10"], ["gate", "12:10", "12:52"], ["review", "12:52", "13:40"], ["run", "13:40", "16:05"], ["gate", "16:05", "16:40"], ["stall", "16:40", "19:28", "held · external op · 2h48 nobody moved"], ["gate", "19:28", "19:49"], ["idle", "19:49", "22:03"], ["stale", "22:03", "22:12"]] },
      { id: "T-854", label: "Co-work C3", segs: [["run", "09:00", "11:40"], ["idle", "11:40", "13:05"], ["run", "13:05", "22:12"]] },
      { id: "T-885", label: "plan P2", segs: [["review", "10:02", "10:33"], ["run", "10:40", "11:58"], ["review", "11:58", "12:10"], ["ask", "12:10", "19:12", "awaiting your acceptance · 7h02"]] },
      { id: "T-868", label: "host-queue-ledger", segs: [["run", "18:58", "21:40"], ["idle", "21:40", "22:12"]], ask: ["19:12", "22:12", "needs you · T-868/R1/Q1 · 3h"] },
      { id: "T-890", label: "pm-bounded-read", segs: [["run", "19:31", "22:03"], ["gate", "22:03", "22:12"]] },
      { id: "T-894", label: "api-connections", segs: [["run", "19:33", "22:06"], ["gate", "22:06", "22:12"]] },
      { id: "T-867", label: "queue-contract-core", segs: [["run", "20:40", "21:52"], ["gate", "21:52", "22:05"], ["stale", "22:05", "22:12"]] },
      { id: "T-871", label: "annotation-service", segs: [["run", "21:17", "22:12"]] },
    ],
    owner: [["10:35", "commented on T-885 P1"], ["12:14", "read T-885 P2 review"], ["19:12", "accepted T-885 P2"], ["21:58", "asked in T-868 thread"]],
    pm: [["09:00", "14:20"], ["14:20", "15:05", "relaunching"], ["15:05", "22:12"]],
  };

  return { NOW, LAST_VISIT, TASKS, GROUPS, HOLDS, Q, HUNK, EXCERPT, LOGTAIL, PLANS, SCALE, CHANGES, AGENTS, USAGE, DEVICES, TL,
    COUNTS: { you: 3, corral: 3, worker: 2, queued: 23, held: 1, parked: 32, backlog: 107, ideas: 55, plans: 6 } };
})();
