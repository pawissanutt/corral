# T-728 — Pi assistant `/resume`: list and safely resume Corral sessions

## Delivered

- **Picker.** In a supervised assistant, `/resume` is a Corral extension command (the packaged Pi
  patch lets the built-in `/resume` step aside when an extension registers it). It reads
  `corral assistant list --json` and shows a searchable list with A-ID, the summary's first line
  (or task focus), last lifecycle activity and availability (`current`, `stopped · resumable`,
  `live · host running elsewhere`, `ended · not resumable`). Empty, unreadable and refused states
  each have an explicit notice. Cancel and refusal append nothing and request nothing. Bridge
  deliveries of `/resume` are rejected `owner_only_command`.
- **Transition.** Selecting a target rechecks the list, writes `resume-request.json`
  (version, assistant, generation, target) in the generation directory, and exits 77. The
  supervisor (`supervise_with`) takes that request only if it names this session and generation,
  then suspends the source (fence `resume another assistant`, control credential and socket removed).
  It then claims the target and runs `assistant::resume`'s checks on it at that moment, so the
  target opens a new generation on its own recorded Pi file, profile and capability snapshot.
  Launch overrides apply only to sessions this `corral pi` opened or named.
- **Claims.** Each supervisor holds a non-blocking exclusive `flock` on
  `$XDG_RUNTIME_DIR/corral/<store>/assistant-claims/A-<n>.lock` while its host runs. A concurrent
  `corral pi --resume` or `/resume` refuses `assistant_claimed`, so a pending launch that has not
  bound can't be stolen. A fresh session (launch and `/new`, via `open_claimed`) is claimed inside
  `create_assistant_with`, under the board lock and before its record is published, so no resumer
  can see it unclaimed, and a launcher that loses a claim publishes and ends nothing (R1 review F1).
  The lock dies with the process.
- **Recovery.** A refused target (ended, unknown, alive, claimed, current, stale request) resumes
  the source with a one-time startup notice (`resume-notice.txt`). A target whose host cannot be
  launched stays resumable, and the owner returns to the source with a notice naming both resume
  commands. A target host that starts and then exits abnormally is not returned automatically;
  `corral pi` prints its resume command and both sessions stay resumable.
- **Rows.** Picker rows are laid out in terminal cells (`rowLines`): the summary is truncated by
  visible width so last activity and availability always remain; at narrow widths they move to
  indented lines (R1 review F2). The packaged Pi patch no longer reports the routed `/resume`
  extension command as a built-in conflict.
- **Quit.** A clean `/quit` suspends the session (fence `quit`), matching main's resumable clean
  exit; a never-prompted session resumes on a fresh file. `corral assistant end` remains the
  permanent end.
  Previously ended records stay ended and are refused `assistant_ended`; nothing revives them.
- **Guidance.** The bound-session refusal now gives role-correct guidance: assistant
  (`/new` / `/resume`, or `corral pi --resume`), PM (`corral agent resume --pm`), worker/reviewer
  (`corral agent resume`). `/fork`, import, and native replacement in other roles stay refused.
- **Docs.** `docs/guide/assistants.md` (switching, claims, quit, compatibility); `cli.md`
  regenerated from the updated `--resume` help.

## Evidence

- `pi_switch_tests.rs`: switch away and return under each identity, with stale generations fenced.
  Ended/missing/live/current/stale/claimed targets return to the source with a notice and leave the
  target untouched. A target that cannot launch returns to the source and stays resumable. Clean
  quit then resume, and explicit end stays ended. `a_fresh_session_is_claimed_before_any_resumer_can_see_it`
  pauses `open_observed` between claim and publication, attempts a rival claim, and proves the winner
  stays open with one generation and that a losing launcher ends nobody else's session.
- Existing `pi_assistant_tests.rs` / `assistant_tests.rs` (/new, relink, resume refusals) still pass.
- `test/assistant-resume.test.ts`: rows/labels/search, selection writes the request, cancel/empty/
  unavailable, refused and revalidated targets, busy refusal, one-time notice, keyboard-driven
  picker render, and cell bounds for a 120-char ASCII and 40-char CJK summary at 80 and 30 columns.
- `scripts/qa-pi-resume.py` drives production `host/main.ts --assistant-new` in a private PTY
  (private HOME/XDG, offline fake bridge, fixture `assistant list --json`; no herdr, owner panes or
  provider calls): `/resume` reaches the Corral picker, search filters, Esc cancel keeps the host,
  conversation and editor with no request or bridge call, the 80x24 layout, a live target refused,
  and selecting A-2 writes the generation-1 request and exits 77. `bun scripts/qa/pi-panel-screenshots.mjs
  <dir> --resume` rasterizes the actual PTY ANSI with xterm.js into candidate-linked PNGs.

## Limitations

The PTY capture covers the host half of the switch; the supervisor half (exit 77 -> suspend, claim,
revalidate, resume) is covered by the Rust supervisor tests rather than a live `corral pi` run. A
draft typed before `/resume` is replaced by the submitted command line, so the picker header promises
only that the conversation stays; text typed after cancelling stays in the editor.
