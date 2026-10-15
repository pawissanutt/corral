# herdr 0.9.3 CLI reference (captured for corral)

> Contributor reference: the herdr CLI shapes corral's `corral-herdr` crate wraps. Not user documentation; see
> [guide/](guide/getting-started.md) for that.

All commands print JSON on success like {"id":..,"result":{..}}. Errors: {"id":..,"error":{"code","message"}} with non-zero exit.
Target a different server with env HERDR_SOCKET_PATH=<path> or `herdr --session <name> <cmd>`.

## herdr workspace create

```
Create a workspace

Usage: herdr workspace create [OPTIONS]

Options:
      --cwd <PATH>
          

      --label <TEXT>
          

      --env <KEY=VALUE>
          Set an environment variable for the launched process

      --focus
          

      --no-focus
```

## herdr workspace list

```
List workspaces

Usage: herdr workspace list
```

## herdr workspace focus

```
Focus a workspace

Usage: herdr workspace focus <workspace_id>

Arguments:
  <workspace_id>
```

## herdr tab create

```
Create a tab

Usage: herdr tab create [OPTIONS]

Options:
      --workspace <WORKSPACE_ID>
          

      --cwd <PATH>
          

      --label <TEXT>
          

      --env <KEY=VALUE>
          Set an environment variable for the launched process

      --focus
          

      --no-focus
```

## herdr pane split

```
Split a pane

Usage: herdr pane split [OPTIONS] [PANE_ID]

Arguments:
  [PANE_ID]
          

Options:
      --pane <ID>
          

      --current
          

      --direction <DIRECTION>
          [possible values: right, down]

      --ratio <FLOAT>
          

      --cwd <PATH>
          

      --env <KEY=VALUE>
          Set an environment variable for the launched process

      --right-click <TARGET>
          [possible values: herdr, pane]

      --focus
          

      --no-focus
```

## herdr pane run

```
Run a command in a pane

Usage: herdr pane run <PANE_ID> <COMMAND>...

Arguments:
  <PANE_ID>
          

  <COMMAND>...
```

## herdr pane read

```
Read pane terminal output

Usage: herdr pane read [OPTIONS] <PANE_ID>

Arguments:
  <PANE_ID>
          

Options:
      --source <SOURCE>
          Terminal snapshot source (default: recent)
          
          [possible values: visible, recent, recent-unwrapped, detection]

      --lines <N>
          

      --format <FORMAT>
          [possible values: text, ansi]

      --ansi
          

      --raw
```

## herdr pane send-text

```
Send literal text to a pane

Usage: herdr pane send-text <PANE_ID> <TEXT>

Arguments:
  <PANE_ID>
          

  <TEXT>
          

next: herdr pane run <PANE_ID> <COMMAND> sends text and Enter in one call
```

## herdr pane send-keys

```
Send key presses to a pane

Usage: herdr pane send-keys <PANE_ID> <KEY>...

Arguments:
  <PANE_ID>
          

  <KEY>...
          

Use esc as the canonical Escape key name; escape is also accepted.
```

## herdr pane close

```
Close a pane

Usage: herdr pane close <pane_id>

Arguments:
  <pane_id>
```

## herdr pane list

```
List panes

Usage: herdr pane list [OPTIONS]

Options:
      --workspace <WORKSPACE_ID>
```

## herdr pane get

```
Show a pane

Usage: herdr pane get <pane_id>

Arguments:
  <pane_id>
```

## herdr pane rename

```
Rename a pane

Usage: herdr pane rename [OPTIONS] <PANE_ID> [LABEL]...

Arguments:
  <PANE_ID>
          

  [LABEL]...
          

Options:
      --clear
```

## herdr pane report-metadata

```
Report display-only pane metadata

Usage: herdr pane report-metadata [OPTIONS] --source <ID> <PANE_ID>

Arguments:
  <PANE_ID>
          

Options:
      --source <ID>
          

      --agent <LABEL>
          

      --applies-to-source <ID>
          

      --title <TEXT>
          

      --clear-title
          

      --display-agent <TEXT>
          

      --clear-display-agent
          

      --state-label <STATUS=TEXT>
          

      --clear-state-labels
          

      --token <NAME=VALUE>
          

      --clear-token <NAME>
          

      --seq <N>
          

      --ttl-ms <N>
```

## herdr pane report-agent

```
Report pane agent lifecycle state

Usage: herdr pane report-agent [OPTIONS] --source <ID> --agent <LABEL> --state <STATUS> <PANE_ID> [-- [RESUME_ARG]...]

Arguments:
  <PANE_ID>
          

  [RESUME_ARG]...
          Command that resumes this session after a Herdr restart; starts with a plain command name

Options:
      --source <ID>
          

      --agent <LABEL>
          

      --state <STATUS>
          [possible values: idle, working, blocked, unknown]

      --message <TEXT>
          

      --seq <N>
          

      --agent-session-id <ID>
          

      --agent-session-path <PATH>
```

## herdr agent list

```
List agents

Usage: herdr agent list
```

## herdr agent get

```
Show an agent

Usage: herdr agent get <target>

Arguments:
  <target>
```

## herdr agent prompt

```
Submit a prompt to an agent

Usage: herdr agent prompt <TARGET> <TEXT> [OPTIONS]

Arguments:
  <TARGET>
          

  <TEXT>
          

Options:
      --wait
          Wait for the first matching state observed after submission

      --until <STATUS>
          State to match after --wait; repeat for more than one state
          
          [possible values: idle, working, blocked, done, unknown]

      --timeout <MS>
          Fail after this many milliseconds

If the agent is already blocked, submission is rejected with agent_blocked before any input is sent. When an accepted submission starts from another non-working state, --wait requires an observed working or blocked state within 5000ms; otherwise it returns agent_prompt_stalled. A caller timeout that expires first returns timeout. It then matches idle, done, or blocked by default, or any exact --until state. It does not track turns: if the agent is already working, that active turn's completion may match.
```

## herdr agent focus

```
Focus an agent

Usage: herdr agent focus <target>

Arguments:
  <target>
```

## herdr agent read

```
Read agent terminal output

Usage: herdr agent read <TARGET> [OPTIONS]

Arguments:
  <TARGET>
          

Options:
      --source <SOURCE>
          Terminal snapshot source (default: recent)
          
          [possible values: visible, recent, recent-unwrapped, detection]

      --lines <N>
          

      --format <FORMAT>
          [possible values: text, ansi]

      --ansi
```

## herdr agent wait

```
Wait until an agent reaches one of the requested states

Usage: herdr agent wait <TARGET> [OPTIONS]

Arguments:
  <TARGET>
          

Options:
      --until <STATUS>
          State to match; repeat for more than one state
          
          [possible values: idle, working, blocked, done, unknown]

      --timeout <MS>
          Fail after this many milliseconds

Without --until, matches idle, done, or blocked. Use --until unknown explicitly when needed. Without --timeout, waits indefinitely.
```

## herdr worktree open

```
Open an existing Git worktree

Usage: herdr worktree open [OPTIONS]

Options:
      --workspace <ID>
          

      --cwd <PATH>
          

      --path <PATH>
          

      --branch <NAME>
          

      --label <TEXT>
          

      --focus
          

      --no-focus
          

      --trust-repository
```

## herdr worktree create

```
Create and open a Git worktree

Usage: herdr worktree create [OPTIONS]

Options:
      --workspace <ID>
          

      --cwd <PATH>
          

      --branch <NAME>
          

      --base <REF>
          

      --path <PATH>
          

      --label <TEXT>
          

      --focus
          

      --no-focus
          

      --trust-repository
```

## herdr notification show

```
Show a notification

Usage: herdr notification show [OPTIONS] <TITLE>

Arguments:
  <TITLE>
          

Options:
      --body <TEXT>
          

      --position <POSITION>
          [possible values: top-left, top-right, bottom-left, bottom-right]

      --sound <SOUND>
          [possible values: none, done, request]
```

## herdr plugin link

```
Link a local plugin

Usage: herdr plugin link [OPTIONS] <PATH>

Arguments:
  <PATH>
          

Options:
      --disabled
          

      --enabled
```

## herdr plugin pane

```
Manage plugin-owned panes

Usage: herdr plugin pane [COMMAND]

Commands:
  open   Open a plugin pane
  focus  Focus a plugin pane
  close  Close a plugin pane
```

## herdr plugin action

```
List or invoke plugin actions

Usage: herdr plugin action [COMMAND]

Commands:
  list    List plugin actions
  invoke  Invoke a plugin action
```

## herdr api snapshot

```
Print the live session snapshot

Usage: herdr api snapshot
```


# Nested commands

## herdr plugin pane open (full)

```
Open a plugin pane

Usage: herdr plugin pane open [OPTIONS]

Options:
      --plugin <ID>
          

      --entrypoint <ID>
          

      --placement <PLACEMENT>
          [possible values: overlay, split, tab, zoomed]

      --workspace <ID>
          

      --target-pane <PANE>
          

      --direction <DIRECTION>
          [possible values: right, down]

      --cwd <PATH>
          

      --env <KEY=VALUE>
          Set an environment variable for the launched process

      --focus
          

      --no-focus
```

## herdr plugin action invoke (full)

```
Invoke a plugin action

Usage: herdr plugin action invoke [OPTIONS] <ACTION_ID>

Arguments:
  <ACTION_ID>
          

Options:
      --plugin <ID>
```

## herdr plugin link (full)

```
Link a local plugin

Usage: herdr plugin link [OPTIONS] <PATH>

Arguments:
  <PATH>
          

Options:
      --disabled
          

      --enabled
```

## herdr agent prompt (full)

```
Submit a prompt to an agent

Usage: herdr agent prompt <TARGET> <TEXT> [OPTIONS]

Arguments:
  <TARGET>
          

  <TEXT>
          

Options:
      --wait
          Wait for the first matching state observed after submission

      --until <STATUS>
          State to match after --wait; repeat for more than one state
          
          [possible values: idle, working, blocked, done, unknown]

      --timeout <MS>
          Fail after this many milliseconds

If the agent is already blocked, submission is rejected with agent_blocked before any input is sent. When an accepted submission starts from another non-working state, --wait requires an observed working or blocked state within 5000ms; otherwise it returns agent_prompt_stalled. A caller timeout that expires first returns timeout. It then matches idle, done, or blocked by default, or any exact --until state. It does not track turns: if the agent is already working, that active turn's completion may match.
```

## herdr pane read (full)

```
Read pane terminal output

Usage: herdr pane read [OPTIONS] <PANE_ID>

Arguments:
  <PANE_ID>
          

Options:
      --source <SOURCE>
          Terminal snapshot source (default: recent)
          
          [possible values: visible, recent, recent-unwrapped, detection]

      --lines <N>
          

      --format <FORMAT>
          [possible values: text, ansi]

      --ansi
          

      --raw
```

## herdr pane run (full)

```
Run a command in a pane

Usage: herdr pane run <PANE_ID> <COMMAND>...

Arguments:
  <PANE_ID>
          

  <COMMAND>...
```

# Observed facts

- `herdr pane process-info --pane <id>` (herdr 0.9.4, 2026-10-15) returns
  `result.process_info` with `shell_pid`, `foreground_process_group_id`, and
  `foreground_processes: [{pid, argv, cmdline, name, cwd}]`. It does not supply process age.
  OmO's foreground group can contain both its `bun.exe .../@code-yeongyu/senpi/.../cli.js`
  harness and helpers such as `bun .../omo-ai/plugin/runtime/ast-grep-mcp/cli.js mcp`.
  The watchdog reads Linux `/proc/<pid>/stat` and `/proc/uptime` for age and start identity,
  and rechecks `/proc/<pid>/cmdline` against herdr's argv before selecting a process.
- herdr detects OMO panes as agent "pi" or "claude" (depends on the backing model); OMO is not an `agent start --kind` value, so corral launches agents with `pane run`.
- herdr's status for OMO panes is wrong: its claude screen heuristics do not match OMO's UI. On 2026-10-15 a working OMO pane (screen `• Running eval (16s • esc to interrupt)`, terminal title `OmO - Running eval`) reported `agent: "claude", agent_status: "idle"` in 6 samples over 6 s; it reported `working` once at startup. corral therefore never trusts one idle sample: idle/done only sets `idle_no_report` after `[sync] idle_after_secs`, and for omo runs a session file under `~/.omo/agent/sessions/<cwd-slug>/` modified within that window counts as working.
- herdr never reports `blocked` for an OMO question prompt. In a probe (herdr session corral-test, 2026-10-15) OMO showed its `Ask user` prompt for ask_user_question while `pane get` returned `agent: "claude", agent_status: "idle"` throughout (45 idle, 4 unknown, 1 `pi`/idle samples; never working or blocked). Only `terminal_title` revealed it: `OmO - Running ask_user_question`. So an OMO run waiting on a question stays Running (eventually with `idle_no_report`); detecting the question from the title or transcript is a follow-up. Evidence: T-014 artifact "herdr status for an OmO question prompt".
- With OmO's herdr state extension (`~/.omo/agent/extensions/herdr-omo-agent-state.ts`) herdr labels OmO panes agent "omo" with accurate working/idle/blocked, but `agent prompt` refuses them with code `agent_not_ready` ("is not an active named agent" / "is no longer the pane foreground process"). corral's `agent_prompt` treats `agent_not_ready` like `agent_blocked`/no-agent codes and falls back to `pane send-text` + `send-keys enter`, so reply and request-changes still reach OmO panes.
- `pane report-agent --state` accepts idle|working|blocked|unknown.
- Plugin runtime commands run with the PLUGIN directory as cwd; the focused pane's cwd is in $HERDR_PLUGIN_CONTEXT_JSON.
- Plugin-free test server: in a subshell, `source scripts/test-herdr.sh; corral_test_herdr_start || exit 1`, then run clients/tests there. The helper isolates HOME/XDG configuration, uses a private `corral-test` socket, and cleans up only its own server. A bare `herdr --session corral-test server` still loads owner plugins; a session name is not plugin isolation. See AGENTS.md for the complete safe-start example.
