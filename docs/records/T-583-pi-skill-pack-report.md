# T-583: Pi-native curated skill pack

## Delivered

Five self-contained skills: frontend, visual-qa, programming, debugging and git-master.
They adapt the OmO originals' useful practices without its tool calls, bundled scripts,
unavailable reference trees or mandatory subagent flows. Visual QA explicitly reports blocked
checks when neither a Pi browser integration nor a repository capture harness is available.

`corral skill install --harness pi-native`, host installation and worker dispatch use the existing
managed installer. The Pi-only root is `~/.corral/skills/pi-native`, added to standard/full default
`skills.user_dirs` alongside `~/.agents/skills`. It is deliberately not an OmO-discovered root.
Managed SKILL.md files carry binary version/content digest stamps; stale managed copies refresh,
foreign skill files and additional owner files remain intact. Explicit directory lists replace
defaults; disabling optional skills and strict behavior remain unchanged.

## Acceptance evidence

| Criterion | Proof |
|---|---|
| Fresh standard worker lists/loads all five | `bun test test/guidance-session.test.ts`: actual HostSession resource registry contains the five names plus corral-worker, no diagnostics, correct file paths; all five `/skill:name` commands expand into delivered user prompts through fixture model turns. Rust capability tests independently verify the default directory list for every role and override/disable behavior. |
| No unavailable tool instructions | The session test parses allowed-tools and inline/call-shaped tool references in each pack file and compares them to that session's actual active worker tool registry. Adding `Use` followed by an inline `not_a_pi_tool` to debugging fails the test; removing the mutation restores green. Pack convention uses bare inline identifiers for tools and full command strings for shell commands. Optional browser operations are schema-discovered, never hard-coded. |
| OmO unaffected | Installer regression creates both .omo and .agents roots, installs/refreshes the Pi pack and the OmO role skills, verifies none of the five pack names exists in either OmO-discovered root, and reports no managed duplicate skills. Existing role-skill dispatch regression also passes. No role pack was added to shared ROLE_SKILLS. |
| Documentation | docs/guide/pi-native.md lists each skill, both installation commands, default path, managed-file behavior, per-role disable/extension settings and browser limitations; docs/guide/skills.md links the isolated pack. |

## Checks run

- `cargo test -p corral-ops skill --lib`: 3 passed (managed pack, existing dispatch refresh and worker skill).
- `cargo test -p corral-core config::agent_capabilities --lib`: 21 passed.
- `cd packages/corral-pi && bun run typecheck`: passed.
- `cd packages/corral-pi && bun test test/guidance-session.test.ts`: 8 passed, 151 assertions.
- Unknown-tool mutation: pack test failed as intended (1 failure); mutation removed and the complete guidance-session file passed again.
- `cargo fmt --all`: completed.
- First quick gate identified the existing all-harness installer test's assumption that every
  harness installs the four role skills. Updated it to verify the five Pi pack skills for the
  Pi-native harness while preserving the four-role assertions for other harnesses.
- `cargo test -p corral-ops --test prompt_ops fresh_install_writes_every_role_for_installed_harnesses_only`:
  passed after that correction (the gate provides its failing-before proof).

- Second quick gate passed Rust checks, then identified the existing cross-lane test's exact
  pre-pack skill list. Extended that integration assertion to include the pack and verify every
  dispatch-installed path and version/digest header.
- In a fresh plugin-free server from scripts/test-herdr.sh,
  `bun test test/guidance-export.cross-lane.test.ts`: 2 passed, 36 assertions. The log's
  PI-PACK-DISPATCH record shows all five skills in the real dispatched worker's default session.

The run's receipted quick gates are recorded separately after commit/rebase.

## Limits

The session probe uses real Pi HostSession/resource loading and a fixture model transport, not a
paid live model. No live OmO session was started: collision safety is tested at its discovery-root
installation boundary. This task ships guidance, not a browser tool; no application UI or screenshot
QA was needed or claimed. Read-only corral search attempts timed out; no expensive investigation
was repeated as a workaround.
