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
  bound can't be stolen. The lock dies with the process.
- **Recovery.** A refused target (ended, unknown, alive, claimed, current, stale request) resumes
  the source with a one-time startup notice (`resume-notice.txt`). A target that cannot start stays
  resumable, and the owner returns to the source with a notice that names both resume commands.
- **Quit.** A clean `/quit` now suspends a session that has a recorded conversation (fence `quit`).
  A session that was never prompted still ends. `corral assistant end` remains the permanent end.
  Previously ended records stay ended and are refused `assistant_ended`; nothing revives them.
- **Guidance.** The bound-session refusal now gives role-correct guidance: assistant
  (`/new` / `/resume`, or `corral pi --resume`), PM (`corral agent resume --pm`), worker/reviewer
  (`corral agent resume`). `/fork`, import, and native replacement in other roles stay refused.
- **Docs.** `docs/guide/assistants.md` (switching, claims, quit, compatibility); `cli.md`
  regenerated from the updated `--resume` help.

## Evidence

- `pi_switch_tests.rs`: switch away and return under each identity, with stale generations fenced.
  Ended/missing/live/current/stale/claimed targets return to the source with a notice and leave the
  target untouched. A target that fails to launch returns to the source and stays resumable. Clean
  quit then resume, and explicit end stays ended.
- Existing `pi_assistant_tests.rs` / `assistant_tests.rs` (/new, relink, resume refusals) still pass.
- `test/assistant-resume.test.ts`: rows/labels/search, selection writes the request, cancel/empty/
  unavailable, refused and revalidated targets, busy refusal, a one-time notice, and a keyboard-driven
  picker render (frames retained as evidence). `host-session` and `assistant-new` still pass.
- All fixtures are isolated: no herdr panes, no provider calls.

## Limitations

The rendered evidence is the picker component's actual output, captured in a fixture TUI rather
than a PTY run through production `main.ts`. There is a small window between `open` and `claim`
for a brand-new session, before its host starts; resume's alive/generation checks still apply there.
