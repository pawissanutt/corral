# T-693 — review round 1 / V1 visual evidence

## Finding and correction

V1 requested candidate-linked rendered screenshots, not a redesign. The initial
submission retained ANSI/input and bridge observations but no screenshot artifacts.
Production UI code is unchanged in this round.

`qa-pi-panel-proof.py` now records the candidate HEAD, initial terminal geometry,
scene byte lengths and actual resize offsets. The companion
`scripts/qa/pi-panel-screenshots.mjs` replays each requested owned-PTY scene through
xterm.js 5.5.0 in a fresh, owned Bun WebView, including terminal resizes in order.
It refuses a capture from a different HEAD, checks that the requested state is
visible in the terminal buffer, validates PNG signatures/dimensions, and writes
`screenshots.json` linking each PNG hash to its ANSI hash and candidate revision.
It closes its browser and loopback server on exit. No mock UI, owner pane or paid
provider is involved. Fixture task/session identifiers in the images are explicitly
owned offline QA data, not a claim to inspect the actual worker pane.

## Reproduction / retained evidence

On the committed, rebased candidate:

```
python3 scripts/qa-pi-panel-proof.py .evidence/T-693/V1-final
bun scripts/qa/pi-panel-screenshots.mjs .evidence/T-693/V1-final
```

The final PNGs, `screenshots.json`, `result.json`, bridge states and complete ANSI
input/output capture are attached to this run. The manifest contains the exact
candidate SHA and both image/source hashes. Prior input artifacts are preserved.

## Cases inspected

Each image is opened with the image read tool, not assessed from ANSI excerpts
alone. The final set is recaptured after commit/rebase and opened again before
submission. There is no product before/after comparison: V1 fixes evidence only.

| Case / PNG basename | Geometry | Verdict | Pixel observation |
|---|---|---|---|
| scoped-agents-wide | 140×46 | PASS | Selected host identity, source/unknown labels, parent-is-not-dependency warning and rail remain legible; editor stays docked. |
| scoped-agents-narrow | 52×24 | PASS | Main list wraps within the viewport; identity and usage are visible; inline rail and editor remain separate. Longer detail requires ordinary inspector scrolling. |
| existing-transcript-wide | 140×46 | PASS | Centered bounded JSONL snapshot is readable, with close/scroll hint and unchanged surrounding inspector/composer. |
| existing-transcript-narrow | 52×24 | PASS | Snapshot wraps at narrow width; header and first records are visible; no session switch or editor displacement. |
| real-job-running | 140×46 | PASS | Blue running selection, armed-local monitor and captured BUILD_WAIT output are distinct and legible. |
| real-job-line-match | 140×46 | PASS | Match observation, settlement-unknown caveat and MONITOR_READY output are visible together. |
| real-job-exited | 140×46 | PASS | Exit 7 is visibly red, group-clear observation and retained output are readable; it is not styled as gate/task success. |

Scope of these verdicts: current packaged dark terminal theme, top-of-inspector
states and centered transcript overlay at the captured widths. The overlay is
intentionally a 90%-width raw JSONL preview; the underlying inspector remains
visible at its side margins. This evidence-only round does not redesign that
presentation. Transcript content beyond the visible page is covered by existing
bounded-reader/scroll input checks, not a claim that every transcript page was
visually audited. No light theme, screen-reader or full accessibility conformance
claim is made. Canonical lost-job reconciliation and cross-host telemetry remain
unavailable as documented in the original report.
