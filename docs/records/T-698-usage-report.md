# T-698 — measured usage and configured budgets

## Delivered

Read-only **Usage** main view and `/panels usage`, with a bounded canonical run-inspection
projection of existing stored collector records. No collector, pricing policy, budget admission,
provider or authority changes. Files repository selection does not alter accounting identity.

| Acceptance | Implementation / proof |
|---|---|
| Task/run/model token, timing and cost facets | Worker coverage, disjoint input/output/cache categories, completed-run wall seconds, model response-latency coverage/mean and price provenance; `cargo test -p corral-ops workbench --lib`: 7 passed |
| Metered/subscription/unknown pricing | Separate transcript-reported and API-equivalent amounts; explicitly not verified bills or subscription charges; missing usage and unpriced counts retained; TS usage tests |
| Configured budget/accounting | Bound host's last canonical receipt; token/USD limits, settled-plus-reserved accounting, unresolved reservations, pricing/legacy/unbounded gaps; unavailable is not unlimited; zero configured limits tested |
| Source-backed chart and compact alternative | Opt-in `c` token-category bars retain exact source tables; compact stacked fields retain all numeric categories; no model recommendations, provider balance or quota |
| Rendering and uncertainty tests | 19 TS tests passed across usage-view, inspection and workbench; TypeScript typecheck passed; draft/conversation preservation, width bounds, stale/unavailable reads and chart toggle checked |

## Actual-input captures

The candidate binary's read-only `run inspect T-698 --run R1` output was saved as
`inspection-source.json` in the attached evidence. It contains real collected T-698/R1
usage (not mock-branch fixtures). The opt-in `CORRAL_USAGE_CAPTURE` test replays that
exact JSON through the production schema and renderer, producing:

- `usage-table.txt`: complete 132-column source table.
- `usage-compact.txt`: complete 46-column stacked source values plus token bars.
- `usage-table-viewport.txt` and `usage-compact-viewport.txt`: production native main-view
  layout frames at those widths, 36 rows tall; the remaining source rows are scrollable.

These are offline SDK-rendered text captures of actual accounting input, not screenshots
of an owner pane or claims of a live interactive provider session. Budget input is absent
in this offline replay and is correctly rendered unavailable; deterministic tests separately
exercise configured limits, interrupted reservations and pricing/legacy uncertainty.

## Checks and limitations

- `cargo check -p corral-ops`: passed.
- `cargo test -p corral-ops workbench --lib`: 7 passed (initial build exceeded tool timeout;
  rerun completed). Existing candidate/proof read-only regressions remain green.
- `bun test src/ui/usage-view.test.ts src/ui/inspection.test.ts src/ui/workbench.test.ts`:
  19 passed, including actual-input capture replay. The first capture test exposed an
  incorrectly sized test viewport; fixed to use the SDK layout frame before the passing run.
- `bun run typecheck`: passed after frozen-lockfile dependency installation.
- `git diff --check`: passed.
- Task-start `cxm context` and cross-task search were attempted but timed out; exact source
  discovery used grep/read thereafter. No analytics collectors were duplicated or modified.

Stored snapshots can lag active usage. Task/run totals explicitly cover workers, not reviews;
model rows cover the selected worker. Missing timing is unavailable, not zero. Up to 100
model rows are shown with an omitted count. Task-less sessions have no fabricated task facets.
Transcript-reported costs are not independently verified invoices. No paid calls, provider quota
checks, load generation or owner/PM-pane QA were performed. Physical-terminal interaction,
full accessibility certification and provider invoice reconciliation are not verified here.
The assigned receipted quick gate is reported separately by Corral after commit and rebase.

## Gate correction

The first quick gate passed formatting, size limits, Clippy and all 2,907 selected Rust tests,
then caught a narrow navigation regression: the new Usage tab caused the Enter hint to be
ellipsized at 34 columns. Shortened compact focus hints without hiding any navigation target.
The existing 34/52/104-column test now also clicks Usage before Graph/Timeline, preserving its
no-ellipsis and full Esc/Enter assertions. Targeted rerun: 26 TS tests passed across usage-view,
inspection, workbench and panel-shell. Typecheck passed separately (the first combined shell
hit its timeout during typecheck). Actual-input source and viewport captures were regenerated
after the hint correction. At that submission no visual screenshot PASS or physical-terminal QA was claimed; the review evidence gap is addressed below.

## Review F1 — owned terminal screenshot and interaction evidence

Added `scripts/qa-pi-usage.py` and a `--usage` mode to the existing terminal screenshot
rasterizer. No production behavior changed in this review round. Reproduction:

```sh
python3 scripts/qa-pi-usage.py .evidence/inspection-source.json .evidence/usage-pty
bun scripts/qa/pi-panel-screenshots.mjs .evidence/usage-pty --usage
```

The source is the previously attached real T-698/R1 collector inspection JSON, not invented
accounting. The driver copies current candidate assets to a temporary directory, uses private
HOME/XDG/config/runtime/session directories, starts production `host/main.ts` on a newly owned
PTY, and supplies only offline fixture auth/bridge transports. It never addresses a herdr pane,
owner session, PM session, or dispatched worker pane. Its one fixture conversation response is
explicitly `FAKE_TRANSPORT_OK`; no provider call is made. The real accounting is independently
replayed through the canonical inspection read seam; fake conversation usage does not replace it.
Budget accounting is genuinely unavailable in this legacy fixture and stays labeled unknown.

The driver sends actual PTY bytes for `/panels usage`, `c` on/off, PageDown/PageUp, resize,
F9, draft text, and the native Usage navigation click. It checks only hello/event calls, exactly
one offline fixture turn, and generation 1 throughout; it removes its process group, assets and
private state on exit. `result.json` records candidate HEAD, exact input bytes, frame byte counts,
resize boundaries and asserted transitions. `screenshots.json` pins ANSI/PNG hashes, dimensions,
candidate and renderer. The established xterm.js 5.5.0 renderer replays the actual PTY output and
resize sequence, validates the visible terminal buffer, checks PNG signatures/dimensions, and
closes its owned browser/server. It does not render reconstructed SDK text or a mock UI.

| Case | Terminal evidence | Observation |
|---|---|---|
| Open wide source/table | `wide-source-table.png` | PASS: complete task/run/model numeric table and uncertainty copy; genuine unavailable budget |
| Chart on | `wide-chart-toggle.png` | PASS: exact table values remain alongside token bars |
| Chart off | `wide-source-restored.png` | PASS: table retained, bars absent (visible-buffer assertion) |
| Budget section | `wide-budget.png` | PASS: explicit unavailable/legacy, not zero; no invented limits |
| Compact chart top | `compact-chart-top.png` | PASS: 52-column stacked source values and bars, dock/navigation retained |
| Scroll to compact end | `compact-budget.png` | PASS: model/source values and unavailable budget reachable with PageDown |
| Retained draft in Usage | `draft-usage-top.png` | PASS: draft remains in composer after native navigation reopens Usage |
| Return to conversation | `draft-return.png` | PASS: same conversation prompt/reply and unsent draft visible after F9 |

All eight PNGs were opened for pixel inspection. Wide captures use 180×38 terminal cells;
compact captures use 52×38. Source rows wrap without dropping numeric categories. The compact
binding/status and input-observation title ellipsize as existing chrome; Usage source content
is scrollable, not silently truncated. No accessibility conformance, physical terminal hardware,
provider invoice or full live-board claim is made. The screenshot evidence supersedes only the
earlier missing terminal-composition/interaction check; earlier functional test results remain
historical checks, and a fresh assigned quick gate is required for this committed candidate.
