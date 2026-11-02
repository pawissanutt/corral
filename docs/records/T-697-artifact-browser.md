# T-697: native Pi artifact gallery and comparison browser

## Delivered

The native workbench adds an Artifacts view (tab when the bar has room, or `a` from
Evidence/F10). It lists the bound task's stored artifacts newest first through a dedicated
bounded projection, `corral run inspect <task> --run <run> --artifacts`
(`crates/corral-ops/src/artifact_gallery.rs`).

Each row carries what the store records: the stable artifact reference, kind, run, title,
registered SHA256, size, added timestamp, compressed/purged state, and whether bytes were
readable at read (legacy snapshot file or live blob). Evidence links come from the bound
run's evidence records that name the snapshot path: criterion, observation key, worker
assertion status, capture time, the candidate HEAD corral registered it against, and
whether the recorded hash equals the artifact's. Unknowns stay explicit:

- Artifact rows record no candidate; only evidence links carry one.
- Other runs' evidence is not read, so those artifacts show "Evidence: unknown".
- An evidence candidate is compared with the bound worktree HEAD by SHA only and labeled
  "same SHA" or "differs · historical"; freshness is never evaluated here. A missing
  worktree yields "bound HEAD unavailable".
- Absent bytes (no snapshot file or live blob) and purged artifacts are labeled, never
  opened.

## Viewers and actions

Rendering, focus and selection never read files or start viewers. Only explicit keys act on
the selected identity:

- Enter on an image: the raw legacy snapshot (confined beneath the store) or the decoded copy
  printed by the existing `corral artifact show` viewer. The bytes are re-hashed and are
  displayed only on a match with the registered SHA256, through the existing native image
  viewer. A terminal without graphics, or undetected dimensions, gets the text/path fallback
  instead.
- Enter on text/reports: the existing `corral artifact show` viewer, bounded to 400 lines,
  control characters stripped.
- Enter on HTML/SVG: path only. `o` (any artifact) opens the browser through the existing
  loopback `corral artifact serve`, invoked with argv and a validated reference.
- `m` selects up to two artifacts, `c` compares them as A/B in selection order: registered
  hash equality and side-by-side provenance. No before/after, candidate order or freshness
  relation is inferred.
- `x` runs the existing `/corral-export` path (same capability gate and journaled
  `session_exported` / `session_export_failed` events); refusals are shown verbatim. No new
  redaction, upload or provider behavior.

## Bounds

One task head read, one newest-first artifact page of at most 100 rows
(`Store::artifact_window`, no full task hydration), at most 8 evidence links per artifact,
128 KiB response cap. Older rows are reported as a gap. Reads happen on entry and `r` only,
never per frame. The per-task catalog needs no historical catalog metadata, so T-672 is not
required.

## Verification

- `cargo test -p corral-ops --lib artifact_gallery`: provenance, unknowns, absent bytes,
  hash mismatch, zero full task hydrations, 101-row bounded page.
- `bun test src/ui/artifact-view.test.ts`: render without viewers, image open with graphics
  and text fallback, decoded-copy image, confinement/symlink/mismatch refusals, report
  viewer, HTML browser only on `o`, comparison selection, export refusal and success,
  stale retention, identity mismatch.
- `scripts/qa-pi-artifacts.py --graphics none|kitty`: owned production-host PTY with real
  keyboard input and resize (140x46 and 52x30), fixture corral recording viewer argv.
  Screenshots are rendered by `scripts/qa/pi-panel-screenshots.mjs --artifacts`.
- Live read-only projection of real board tasks with the worktree-built binary.
