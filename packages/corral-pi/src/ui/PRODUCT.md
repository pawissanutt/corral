# Corral corral-pi workbench

Platform: terminal UI. Production rich sessions run the existing Pi InteractiveMode and HostSession; there is no browser runtime or web service.

Confirmed owner direction (A-9/g1/i8–i14): OpenCode-inspired panels, real working UI/backing, responsive sidebar that becomes inline metadata, and preservation of draft/selection. Owner explicitly requested implementation in the actual TUI rather than more mockups or filing alone.

Primary action: compose and inspect live work. Information priorities: conversation/input, the session's task and current state, then contextual resources/progress. The workbench has native layout, context/plan/job/auxiliary inspection, persistent searchable Changes and Agents main views, captured hunk-to-draft references, click-to-expand terminal images and image preview of artifacts (owner inputs i17–i19). One trusted local owner: the workbench keeps the live conversation and its draft in the same session.

Task-bound sessions (worker, reviewer) read their task, runs, checks and artifacts from the engine's `corral show --json` and `corral artifact list --json`: a committed comparison of the run (base → head), exact identities, file diffstats and real hunks, and F10 Proof with the newest gate and review checks and the task's artifacts. No acceptance is synthesized. Sessions without a task (assistant chat, build) have no Task or Proof; alternate repository inspection never rebinds the session.

The owner can rewind a worker, chat or build to the start of an earlier turn from F2 and see what the session used on Usage. Worktree rewinds save the current work under a rescue ref. Authoritative data stays engine- and Pi-owned. No simulated completion, role switching or permission controls.
