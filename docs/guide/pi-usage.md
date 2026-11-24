# Pi usage inspection

Open **Usage** from F2 (Views) or `/panels usage` (see [native panels](pi-panels.md)). It is read-only, not a tab (its place reads `Chat › Usage`) and stays bound to the launched session, and to its task and run when it has them, even when the Files repository changes. F9 returns to the same conversation; the editor is not replaced. Scroll with arrows, paging or wheel; `?` shows the reading rules. There is no other entry: no function key and no click on the status bar's cost or context.

The page is four independent sections, each with its source and freshness on its own rule. They are never added together, and an unknown value is never drawn as zero:

1. **Session**: what this pane's own session used, live.
2. **Run budget**: where the host's accounting stood at its last receipt.
3. **Task / run**: the stored collector facets of the bound task run.
4. **Diagnostics**: the last receipt's reservations, pricing and legacy gaps and stop state, shown only when a receipt was read.

Views shorter than 12 body rows open with a three-row digest of the first three sections; the full sections follow it.

## Session

Tokens, the API-equivalent USD estimate, the context window (`48,000 / 200,000 · 24%`) and the model with its effort. An unknown value reads `unknown` (a context share without a known window reads `percent unknown`). A build session lists this pane and each of its build agents as rows with a `total` row; the total cost is the status bar's rule (the parent plus the agents' known costs), and a `+` beside a total marks a member whose value is unknown. Helper calls (Explore) are neither listed nor added, so no child's usage is counted twice. Session tokens and cost are counted by this pane and priced from the model catalog: an estimate, not a bill or a provider quota.

## Run budget

The bound host's last accounting receipt, separate from the session figures. Its rule names the source (`last host receipt`, or `local host receipts` in a build session) and the section states exactly one condition:

- **Read.** The configured token and USD limits, each as a meter next to the **accounted** total, which is settled plus reserved (accent, warning from 80%, error from 95%). A limit that is not configured reads `not configured`, never unlimited. A receipt without limits reads `no limit configured` beside the accounted totals. A build session's local control plane keeps settled and reserved apart, so both rows follow the accounted sum there.
- **Unavailable**: `No budget receipt from this host · limits and totals unknown, not zero`. The cause is not guessed.
- **Unreadable**: `Budget receipt could not be read`, in warning.
- **Older host generation**: `This host generation predates budget accounting`, said only when the host's capability record proves it, never because a receipt is merely absent.

What the receipt recorded about stopping is stated as of that receipt: `host stopped new requests at last receipt`, `not stopped at last receipt` or `stop state not recorded`, with the requests in flight when it recorded them. That is neither a claim that the host runs now nor that it stopped now; host liveness is a separate fact (the status bar's binding).

The accounted USD is canonical reservation accounting, not billed spend, remaining subscription balance or provider quota. Cancellation is best-effort and provider billing may lag. A limit-reached row names whether the host had stopped.

## Diagnostics

From the same receipt, in plain counts: unresolved reservations (retained and still counted) and unresolved unbounded requests, pricing gaps (unknown pricing is not free), the legacy usage gap (historical spend unknown), an uncertain-spend flag when the receipt marks it without a count, and the stop state above. While any of them is not clean, a warning row also closes the budget section. A local receipt records none of these counts and says so.

## Task / run

The source table comes from existing stored collector snapshots, only for a task-bound session that has a reader. Task totals cover worker runs (including missing-run coverage); selected-run model rows derive from that worker's response entries. Reviews are explicitly excluded. Up to 100 model rows are shown, with any omitted count retained. Source strings retain pricing provenance.

PM, assistant and build sessions hold no task run. The section is one neutral row, `Does not apply: this <role> session has no task run.`, stated once. Nothing is read: opening Usage, the page's timer and `r` never call a collector, and the footer offers neither `r` nor `c`. A session with a task run but no stored-accounting reader says that instead. No cross-session attribution is invented.

A source has one read state, worded the same as the Task, Proof, Inbox, Graph, Timeline and Artifacts views: `reading…`, `not read yet`, `! read failed` (the reason, and no rows), current (`read 12:04 (2m ago) · r reread`) and `! stale · 12:04` for retained rows, which stay on screen with the time they were read and, after a failed reread, the reason and the time it failed. A host that is not bound is a separate `! host unbound` flag beside the state, never in place of it. A record from an older collector shows `Collected usage not recorded by this run record · unknown, not zero`.

Active responses can lag; missing usage, legacy model or timing evidence, failed reads and retained stale data are not zero. Completed wall time is not model-active time; response latency is the collector's transcript-derived latency, not throughput.

Reported USD is transcript-reported cost, not independently verified billed metered spend. API-equivalent estimates are separate: subscription usage does not imply an extra subscription charge. Unpriced token counts and partial worker coverage remain visible. The UI neither reprices history nor decorates or recommends models.

`c` toggles token-category bars under each source row, while stored model rows are shown. The exact numeric tables always stay, including on compact terminals (columns that do not fit move to a muted row under their scope); bars are relative to the largest category in that row, not a provider quota or a percentage of a budget. `r` rereads canonical stored accounting only; it does not run a collector or contact a provider.
