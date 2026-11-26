# Session control from the app

The daemon controls agent conversations by session id, not pane id. The interactive
surface is a chat with a transcript and live events, not a byte-stream terminal.

## Controls

Use the Agents row id for a task run (for example `T-001_R1`),
`T-001/reviewer` for the current reviewer, or `A-1` for chat.

| Control | Daemon request | Effect |
|---|---|---|
| Cancel | `session.cancel {id, turn?}` | Stops the current turn without shutting down the host. |
| Pause | `session.pause {id}` | Stops the turn and holds automated input. The host stays alive. Not offered for reviewers. |
| Resume | `session.resume {id}` | Delivers held input in order. An owner message also resumes the session and goes after the held input. |
| Terminate | `session.terminate {id}` | Stops the addressed host. A task's worktree and branch stay; its run is stopped and the task is Blocked. A chat's transcript stays readable. |
| Relaunch | `session.relaunch {id}` | Retries a worker or starts review again; chat reopens its existing session. Use the returned `id` for the resulting session. |
| Rewind | `session.rewind {id, action:"list"}` or `{id, action:"turn", turn}` | Lists or restores worker checkpoints. The host saves current edits under a rescue Git ref before restoring the tree and transcript. |
| Answer a Pi question | `prompt.answer {session, question, option?, text?}` | Answers the named question with a choice, text, or both. |

Pause is a host state, not a task hold. A gate can still run; its send-back waits
in the paused worker's `session.json` until Resume or owner input. Pause and held
input survive restarting the host. A task reply uses `op.submit` with `Op::Reply`.
Without a Pi socket, Cancel and Pause are refused; Terminate can still stop a
running non-Pi task session.

There is no separate PM. Send an `assistant.send` to `thread:"quick"` to start or
continue the headless quick chat. No automation setting is needed. Terminate lets
you relaunch the same chat; End (`assistant.session {op:"end", session}`) preserves
the transcript but the next quick message opens a new session.

Local and paired owners use the same commands. Confirm Terminate or Rewind once
in the client; the daemon does not issue previews or proofs. Do not queue controls
while offline.

The replay and live-topic sections below preserve the earlier app design notes.
Use `corral-protocol` for the current wire types.

## Transcript replay

The replay source is the recorded Pi session, not the pane's current screen. All roles
use the same reader. A session file is an append-only JSONL tree: replay follows the
latest complete entry's `parentId` chain, including history before compaction, and
foreign harness files without that tree metadata retain append order. Partial final
lines remain invisible. The reference is Pi's reopen path:
`pi-coding-agent/dist/modes/interactive/interactive-mode.js:3780-3784`
(`renderInitialMessages` → `getBranch` → `renderTranscriptHistory`), not its
compacted model context.

### Sources and serialized items

In the table, `pi/` means
`packages/corral-pi/node_modules/@earendil-works/pi-coding-agent/dist/`, and `corral/`
means `packages/corral-pi/src/`. These are source citations; the installed Pi distribution
is the authority inventoried here. Content is retained before visual folding, clipping,
syntax highlighting or Markdown layout. A renderer may collapse it as Pi does, but
must offer the complete recorded content through expansion/continuation.

| Terminal element | Session-file source | Serialized `SessionItem` / fields | Drawing authority |
|---|---|---|---|
| Owner and assistant Markdown, code fences, tables, skill invocation text | `message.message.content` string or `text` blocks; foreign top-level `type: user/assistant`, `input_text`, `output_text` | `text.text`; no Markdown is flattened | `pi/modes/interactive/interactive-mode.js:3482-3524`, `components/user-message.js:31-42`, `components/assistant-message.js:140-197` |
| Thinking, including reasoning hidden by the terminal's toggle | Assistant `thinking.thinking` blocks | `thinking.text` | `pi/modes/interactive/components/assistant-message.js:153-195`; `corral/ui/DESIGN.md`, Transcript |
| Tool row and its expanded arguments/result | Assistant `toolCall` or foreign `tool_use`; later `toolResult` / `tool_result`, matched by `id` ↔ `toolCallId` / `tool_use_id` within the returned page | `tool_call.call_id`, `name`, `arguments` (pretty JSON), `summary` (first command/path/target line), `state`, `paired`, `call` (complete call JSON), `output` (text), `result` (complete result JSON), `elapsed_ms` (`durationMs` when recorded) | `pi/modes/interactive/interactive-mode.js:3588-3643`; `corral/ui/tool-row.ts`, `tool-rendering.ts` |
| A call without its result, or result without its call | Either half outside the current page / no matching recorded id | `tool_call.paired: false`; a call has `result` absent, a result-only row has empty `arguments`; states are `running`, `done`, `failed`, `not_run` | Same tool lifecycle; no name is invented from a call id |
| Read, grep, find, ls, highlighted file and listings | Call arguments and result content/details | `tool_call.arguments`, `output`, `result`; original `path`, `file_path`, ranges, counts, truncation notices and images remain available | `corral/ui/tool-lines-files.ts:69-88,170-218` |
| Edit diff, patch and replacements | `arguments.edits` / `oldText` / `newText`; `result.details.diff` or `.patch` | `tool_call.diff`, `arguments`, `result` | `corral/ui/tool-lines-files.ts:90-141` |
| Write source/new content | `arguments.content` | `tool_call.content`, `arguments` | `corral/ui/tool-lines-files.ts:143-156` |
| Bash/managed command and job rows, exit state, output tails, saved output location | Tool arguments/result, `corral_command`, `corral_command_monitor`, `corral_job_control` journals | `tool_call` with complete `result`; supplemental `card.title` equal to the journal's `customType`, `data` equal to its complete pretty-JSON `data` | `corral/ui/tool-lines-jobs.ts`; `host/session.ts` command/monitor restore |
| Corral operation rows, receipts, refusal reasons, expanded fields, plan progress | Corral tools' arguments and complete result, including receipt envelope and structured `details` | `tool_call.arguments`, `call`, `result`, `output`; `summary` is the recorded call target, not a fabricated receipt verdict | `corral/ui/corral-presenter.ts:444-824,854-902`; `tool-rendering.ts:48-52` |
| Diagnostics, definition, references, fetch, web read/search, browse/screenshot, ask, explore/oracle, experts, MCP and unknown tools | Their complete call/result | `tool_call`; every presenter-specific field remains inside `call` / `result`, including nested tool records, usage and images | `corral/ui/tool-lines-local.ts`, `tool-rendering.ts:144-169` |
| Build task, finish and board-plan tool cards | `task`, `build_finish`, `board_plan` call/result | `tool_call.arguments`, `call`, `result` | `corral/ui/tool-rendering.ts:51-52` |
| Code Mode cell source, child calls/results, error, printed/returned values, detach/settle status and timing | `codemode` arguments and `result.details.cells`; later `corral_code_call` and `corral_code_cell` snapshots | `tool_call.arguments`, `call`, `result`; `card.title: corral_code_call/corral_code_cell`, `data` complete snapshot JSON; child records are not reduced to a single output string | `corral/ui/code-cell.ts:140-179,231-249,383-509`; `host/session.ts` CodeRuntime journal/restore |
| Owner `!` / `!!` execution | Message role `bashExecution` | `bash_execution.command`, `output`, `exit_code` (absent is unknown, never zero), `cancelled`, `truncated`, `exclude_from_context`, `full_output_path` | `pi/core/messages.d.ts:16-27`; `corral/ui/owner-bash.ts` |
| Images, including tool result images | `image.data`, `mimeType`; foreign base64 `source.data` / `source.media_type` | `image.media_type`, `size` (decoded byte count), `encoding: base64`, `data`; large images use the same item continuation as text | `pi/core/messages.d.ts`, `pi-ai/dist/types.d.ts:275-279`; `corral/ui/DESIGN.md`, Images |
| Compaction divider and expandable summary | Entry `compaction.summary/tokensBefore`, or role `compactionSummary` | `compaction.summary`, `tokens_before` | `pi/core/session-manager.d.ts:46-59`; `pi/modes/interactive/interactive-mode.js:3466-3471`; `corral/ui/DESIGN.md`, Compaction and long sessions |
| Branch summary | Entry `branch_summary`, or role `branchSummary` | `branch_summary.summary`, optional `from_id` | `pi/core/session-manager.d.ts:60-70`; `pi/modes/interactive/interactive-mode.js:3473-3478` |
| Summary billing notice | Compaction/branch entry `usage` | `card.title: Summary cost`, `data` with the complete `usage` | `pi/modes/interactive/interactive-mode.js:3678-3679,3701-3713` |
| Cache-warming usage notice | `usage` entry with `kind: cache_warm` | `card.title: Cache warming`, `data` complete entry (provider/model/note/usage) | `pi/modes/interactive/interactive-mode.js:3685-3699`; other usage categories are not transcript rows |
| Model or effort change | `model_change.provider/modelId`, `thinking_level_change.thinkingLevel`; richer `corral_mark.kind: model` | `model_change.provider/model`; `thinking_level_change.level`; `card.title: corral_mark`, `data` retains `model`, `effort`, `was`, `at` | `pi/core/session-manager.d.ts:27-35`; `corral/host/transcript-marks.ts:36-42`, `ui/mark-rows.ts:120-136` |
| Label/bookmark | `label.targetId/label` | `label.target_id`, optional `label` (absence removes a label) | `pi/core/session-manager.d.ts:86-91`; labels are tree/bookmark UI facts, included even though `sessionEntryToContextMessages` draws no chat row |
| Aborted/not-run turn | Assistant `stopReason: aborted`, `errorMessage`, `providerFailure`; tool call of that assistant turn without a result | `aborted.message`, optional `facts` (complete pretty JSON), `tool_call.state: not_run` | `pi/modes/interactive/interactive-mode.js:3535-3560`; `components/assistant-message.js:208-225`; `corral/ui/error-copy.ts` |
| Provider error or reply cut at the output limit | Assistant `stopReason: error/length`, `errorMessage`, `providerFailure` | `provider_error.message/facts`; `card.title: Reply cut`, `text`; failed unexecuted calls have `state: failed` | `pi/modes/interactive/components/assistant-message.js:198-245`; `corral/ui/error-copy.ts` |
| Turn footer and diagnostic facts | Assistant metadata excluding `content` | `SessionMessage.model`, `stop_reason`, `usage` (original token/cost object); `card.title: Turn footer`, `data` retains **all** original metadata, including `diagnostics`, provider failure, model/provider/response ids, effort, raw stop reason and `durationMs` | `pi-ai/dist/types.d.ts:294-322,384-415`; `corral/ui/usage-format.ts`, `ui/error-copy.ts` |
| Run-end mark (done/waiting/stopped/error/length/continue-stopped), waits, counts, duration/cost/rebilled tokens | `custom.customType: corral_mark`, `data.kind: turn_end` | `card.title: corral_mark`, `data` retains the whole typed mark, including `notRun`, `waits`, `note` | `corral/host/transcript-marks.ts:14-34`; `ui/mark-rows.ts:76-118` |
| Host note and owner/child asked/answered/steered mark | `corral_mark` with `kind: note/child` | `card.title: corral_mark`, `data` retains every field/text | `corral/host/transcript-marks.ts:44-61`; `ui/mark-rows.ts:138-158` |
| Delivered code-cell/job/inbox/Corral host notices and host prompt rows | Aggregate user text beginning `[corral `, `[corral_`, `[corral] `, `[command `, `NEW REVIEW ROUND `; host kickoff/session-prompt and relink shapes recognized by `host_input`; owner-input-tagged text is excluded | `card.title: Host notice`, `text` complete joined text; renderer can reproduce the notice classifier without losing expanded text | `corral/ui/host-notices.ts:35-155,191-223`; `ui/host-envelopes.ts:240-317`; `session_replay.rs::host_input` |
| Tagged owner input and `ask_user` answer bubble | Ordinary owner/user text, including `[owner input …]` or `[ask_user answer …]` | `text.text` complete original; a renderer can remove framing for presentation, not from the data | `corral/ui/host-notices.ts:157-189,246-313` |
| Deferred owner questions and answers | `corral_owner_question.data` snapshots | `card.title: corral_owner_question`, `data` keeps id, questions/options, status, answers, expiry and delivery state | `corral/host/pending-questions.ts:22-26,61-83`; `ui/owner-questions.ts` |
| Displayed extension custom message, including build owner exit/board/finish cards | Entry `custom_message` or role `custom`, `display: true` | `card.title` is `customType`, `text` is the full textual content, `data` is the complete message/entry including `details`; image blocks also have `image` items | `pi/modes/interactive/interactive-mode.js:3457-3464`; `components/custom-message.js:50-93`; `corral/ui/build/build-entries.ts:20-27,149-170` |
| Canonical assistant owner/assistant/PM words and cards | Canonical `ThreadEntry`, not duplicated from Pi messages | `text`; existing `question_card`; other cards as `card.title: Owner card`; links/attachments as `card.title: Attachments`; canonical model/cost retained in `Turn footer` | `crates/corral-ops/src/agent/app_runtime/projection.rs:322-368` |
| Existing compatibility tool item | Old app wire only | `tool { tool: ToolLine }` remains defined but is never emitted by replay | The app migrates to `tool_call` |

`ToolCall.arguments`, `call`, `result`, and `Card.data` are JSON **strings**, not
flattened field/value summaries. Decode them as JSON after resolving any continuation.
An absent result remains unknown/running unless the recorded assistant stop says the
call did not run or failed. Matching never treats an id as a tool name. A result folded
into a call is not emitted a second time; its image items remain available.

The supplemental cell/job/question cards preserve journal snapshots used to rebuild
their live TUI models; clients may coalesce snapshots by their recorded cell/job/question
identity. Build message cards retain the original `details` instead of guessing a live
outcome. An item's expanded recorded content is the source of truth, not its preview.

### Intentionally invisible entries

The entry inventory is `pi/core/session-manager.d.ts:5-126`; the role inventory is
`pi/core/messages.d.ts:16-58` and `pi-ai/dist/types.d.ts:379-451`.

* `session` is file/header metadata, not a transcript entry. `session_info` renames
  the session/header; `context_edit` changes model context, not past visible words.
  None yields chat content in `sessionEntryToContextMessages`
  (`pi/core/session-manager.js:166-191`). Original history stays readable after
  compaction (`interactive-mode.js:3780-3784`).
* Role `system` is deliberately not displayed
  (`pi/modes/interactive/interactive-mode.js:3480-3481`). This includes a compaction's
  stored `systemMessage`; its visible summary is still replayed.
* `custom_message` / role `custom` with `display: false` is hidden by the terminal
  too (`pi/core/session-manager.d.ts:105-115`;
  `pi/modes/interactive/interactive-mode.js:3457-3464`).
* `usage` categories other than `cache_warm` are accounting, not transcript rows
  (`pi/modes/interactive/interactive-mode.js:3670-3673`).
* Plain `custom` entries draw only with a registered entry renderer
  (`pi/modes/interactive/interactive-mode.js:3427-3435`). Corral's inline renderer is
  `corral_mark` (`corral/ui/mark-rows.ts:330-340`). Cell/job/question journal snapshots
  needed by other Corral surfaces are retained as the supplemental cards above.
  The other journals appended by `corral/host/session.ts` are intentionally invisible:
  `corral_review_base` (review baseline), `corral_hook` (hook state),
  `corral_checkpoint` (recovery context), `corral_effect_lifecycle` (effect recovery),
  `corral_cost_turn` (cost attribution), `corral_host_bound` (binding provenance).
  Delivery/fixture-delivery records restore delivery identity, not a second copy of
  their user-message envelope (`host/session.ts`, delivery restore).
* Text/thinking signatures and image/transcript layout controls are not prose:
  the call/result and footer metadata retain presenter inputs, but opaque provider
  thinking signatures are not drawn (`components/assistant-message.js:140-197`).

### Bounds and continuation

The JSON-RPC reply, not merely its strings, must fit `APP_CONTROL_MAX` = 60 KiB.
The reader scans backwards through 128 KiB windows to find complete record boundaries,
then streams **one record** through a buffered JSON decoder; it never loads a whole
session file. A record may exceed a window. Work per history page is at most 32
records (including invisible/off-branch entries), rather than a whole-history scan.
Allocations for a selected record are proportional to that record; this is not a
constant-memory streaming JSON-field parser.

Each emitted item is at most 2,048 serialized bytes as a preview. If the complete
item is larger, its preview is followed immediately by
`{"kind":"continuation","token":"…","bytes":N}`. That token belongs only to the
preceding item. Previews can contain clipped pretty-JSON strings and incomplete
base64: do not parse/decode these as complete payloads while a continuation follows.
Every original block remains pageable: item-count limits never discard the tail of
a message. `SessionPage.older` addresses older records or the earlier items of the
same record, and is passed back unchanged as `DetailGet.page` for the same session.
Items of a split record retain their original order across the older slices.

For the complete item, send
`{"kind":"session_item","id":"<session id>","token":"<item token>"}`
as the flattened `DetailGet.target` (with `etag` and `page` null/absent).
The response is `DetailBody` kind `session_item`, with body:

* `json`: at most 4,096 UTF-8 bytes of the complete serialized **one `SessionItem`**;
* `offset`: byte offset of this fragment in that serialized item, starting at zero;
* `bytes`: total UTF-8 byte length of that serialization, constant for this chain;
* `next`: token for the immediately following fragment, absent on the final one.

Concatenate the decoded `json` strings in order, requiring each `offset` to equal
the number of UTF-8 bytes already assembled. Request each `next` using another
`session_item` selector with the **same session id**, not `DetailGet.page`.
At completion the byte count equals `bytes`; deserialize the concatenation as
`SessionItem` and replace the preview plus its `continuation` item. Fragments are
not independently JSON documents. This applies unchanged to images: the assembled
item is `image`, with `encoding: base64`; only then decode its complete `data`
and check the decoded size against `size`. No image path or URL is accepted from
the client.

Tokens authenticate the session id, source record/item, paired-result record/item,
content digest and fragment offset with process-keyed HMAC-SHA256. A cross-session,
modified, stale-content or previous-daemon-process token is refused before using
its source offsets. The server resolves paths only from the session's trusted
record. The client must reload after a daemon restart instead of interpreting tokens.

Normal transcript payloads target 28 KiB, reserving up to 4 KiB per selected tool
for post-selection pairing. Assistant pages allocate 12 KiB each to canonical thread
content and supplemental Pi facts. Message metadata is separately bounded; full
assistant metadata stays in the continued `Turn footer` item. Session identifiers
and row metadata have bounded previews. For worst-case escaping, a 4 KiB item fragment
can expand to at most 24 KiB of JSON string content, plus a token below 2 KiB and
fixed metadata. History payloads plus bounded row metadata remain below 52 KiB.
`paging::reply` and the daemon's `views::bounded` independently measure serialized
replies against `APP_CONTROL_MAX - 2048`, reserving the JSON-RPC envelope.

When a canonical assistant thread exists it remains the authority for owner/assistant
words, question cards and attachments. Its recorded Pi file supplies tools, thinking,
images, notices, markers and footer/diagnostic facts. Pi owner/assistant `text` items
are suppressed there, and legacy canonical tool summaries are suppressed when Pi
is available, so neither words nor calls appear twice. The two append-only sources
have independent positions inside the opaque older cursor. Merge loaded pages by
their `at` timestamps; do not assume their time ranges cannot overlap.

### Not in a replay

* `quietNote` rows deliberately exist only in the live transcript component
  (`corral/ui/mark-rows.ts:8-9,266-279`), not in the session journal. Reopening also
  loses these rows: `renderInitialMessages` rebuilds from `getBranch()` and
  `renderSessionEntries` only converts persisted entries
  (`pi/modes/interactive/interactive-mode.js:3665-3683,3780-3784`); it does not
  consult a separate quiet-note store.
* Cache-miss labels/cost estimates depend on earlier usage, the live model catalog
  and display settings (`pi/core/cache-stats.js:14-40`;
  `pi/modes/interactive/interactive-mode.js:3565-3569,3765-3778`). All recorded usage,
  provider/model, timing and diagnostic inputs are retained in `Turn footer`; an
  exact historical live catalog price/setting is not recorded or fabricated.
* In-flight streaming deltas, expansion/toggle state, animation clocks, viewport
  folds and transient dialogs/toasts have no durable session entry. Recorded
  settled calls, owner question snapshots and displayed custom cards are replayed;
  a past unrecorded frame is not.

### Rust wire definitions

These are the replay type definitions in
`crates/corral-protocol/src/detail/session.rs`; variant/field spellings below the
Serde rename attributes are the renderer's binding contract.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMessage {
    pub author: String,
    pub at: Option<String>,
    pub items: Vec<SessionItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionItem {
    Text {
        text: String,
    },
    Tool {
        tool: ToolLine,
    },
    QuestionCard {
        key: String,
        rev: u32,
        text: String,
        options: Vec<QuestionOption>,
        answer: Option<QuestionAnswer>,
    },
    Thinking {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        /// Pretty JSON, not a flattened argument description.
        arguments: String,
        summary: String,
        state: SessionToolState,
        paired: bool,
        /// Original call metadata, including argument validation errors and namespaces.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        /// Complete structured result, including images and presenter-specific details.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    BashExecution {
        command: String,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i64>,
        cancelled: bool,
        truncated: bool,
        exclude_from_context: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        full_output_path: Option<String>,
    },
    Image {
        media_type: String,
        size: u64,
        encoding: String,
        data: String,
    },
    Compaction {
        summary: String,
        tokens_before: u64,
    },
    BranchSummary {
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_id: Option<String>,
    },
    ModelChange {
        provider: String,
        model: String,
    },
    ThinkingLevelChange {
        level: String,
    },
    Label {
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Aborted {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        facts: Option<String>,
    },
    ProviderError {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        facts: Option<String>,
    },
    /// All persisted fields read by an extension presenter, without reducing them to prose.
    Card {
        title: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    /// Immediately follows the preview it replaces. Fetch and concatenate JSON fragments.
    Continuation {
        token: String,
        bytes: u64,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionToolState {
    Running,
    Done,
    Failed,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionItemPage {
    /// A UTF-8 fragment of the complete serialized SessionItem.
    pub json: String,
    pub offset: u64,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
```

`DetailRef` adds `SessionItem { id: String, token: String }`, serialized with
`kind: session_item`. `DetailBody` adds `SessionItem(SessionItemPage)`, serialized
as `{"kind":"session_item","body":{...}}`. Existing `Text`, `QuestionCard` and
legacy `Tool` remain wire-compatible, and old clients can decode new item kinds as
`Other`. Optional new fields default to absent.

## Live session topic

`session.v1` advertises the scoped `session` topic on both authenticated app carriers.
Open with `sub.open {topic:"session", scope:<AgentRow.id>, since:null}`. Scope is the
public session id (`pm`, `A-<n>`, or a task session's Agents row id), not a pane id
or an inferred container key. A host without a healthy corral-pi bridge or without
`hello.commands` advertising `live_since` leaves the subscription open with no live
events; it never produces a connection-level error. Ops exposes stable `no_bridge`
and `live_unsupported` refusal codes to its caller. Pane-hosted sessions use the
recipient's generation directory; daemon-hosted assistants use their assistant
runtime directory. Reading neither writes the store nor sends input.

### Delivery and client rules

- Each scope has exactly one shared host reader/cursor, regardless of subscriber
  count. One dedicated `corrald-session-live` thread performs bridge I/O, independently
  of the socket, app read worker, Thread topic and assistant supervisor. It checks
  `Shared.closed` between scopes, and daemon cleanup joins it after closing Shared.
  A scope's final close/drop releases its buffer and reader; no unsubscribed scope
  performs `live_since` calls.
- A healthy idle tick is one `live_since` call, with hello capability negotiation
  cached on the connection. A quiet scope is read every `SESSION_POLL = 100 ms`, never
  faster than 10 Hz. A scope whose host returned events within the last
  `SESSION_STREAM_LINGER = 1 s` is read every `SESSION_STREAM_POLL = 30 ms`, and a full
  page is followed at once (at most `SESSION_STREAM_BURST = 8` times running, so one
  backlog cannot hold the shared worker), so a reply being written reaches its watchers
  in steps too short to see. The worker sleeps until the next scope is due. A failed bridge is
  re-resolved on a later tick; subscriptions survive termination, loss and relaunch.
- A page buffered for a scope wakes each connection that watches it (`wake::Waker`), on
  both carriers, so it is sent when it is published and not when that connection's idle
  read slice (500 ms on the socket, 250 ms on the WebSocket) runs out. An idle read
  wakes nobody. The wake stays pending until the connection's loop takes it as it
  flushes, and goes before client input that is ready at the same time.
- Limits in `topics/app/session.rs`: `SESSION_SCOPES = 128` watched scopes globally,
  `SESSION_SUBSCRIPTIONS = 32` total app-topic subscriptions per connection,
  `SESSION_SCOPE_BYTES = 512`, `SESSION_BUFFER_EVENTS = 512`,
  `SESSION_BUFFER_BYTES = 512 * 1024` serialized event bytes per scope,
  `SESSION_PAGE_EVENTS = 32`, `SESSION_ENVELOPE_BYTES = 4 * 1024`, and
  `SESSION_PAGE_BYTES = APP_CONTROL_MAX - SESSION_ENVELOPE_BYTES` (56 KiB of event
  JSON including separators). The full JSON-RPC `sub.delta` notification is checked
  against `app::APP_CONTROL_MAX = 60 * 1024`, not just its event array.
- Limits in ops `session_live.rs`: `LIVE_IO_TIMEOUT = 250 ms` per socket read/write,
  `LIVE_HOST_EVENTS = 512` accepted events per host page, `LIVE_EVENT_BYTES = 8 * 1024`
  per event, `LIVE_PAYLOAD_BYTES = 1024` for each text/thinking chunk, args or partial
  output, and `LIVE_ERROR_BYTES = 512`. The bridge additionally enforces
  `FRAME_MAX_BYTES = 64 * 1024`. The host's separate live ring retains 512 events;
  its existing page reader returns at most 256 events and one frame.
- There is exactly one page in flight per subscriber. Ack with
  `sub.ack {topic:"session",scope,cursor}` using the exact delivered cursor. An
  incorrect or duplicate ack does not advance the stream. `epoch` is the daemon's
  normal topic epoch. `cursor.rev` is the last delivered host sequence (or the
  resume point for an empty gap page); `cursor.after` is an opaque
  `session:<buffer-revision>:<page-serial>` token. Buffer revisions are unique across
  scope recreation within the daemon. Pass an acknowledged cursor and epoch in
  `since` to resume only while that buffer is still compatible. `more` means the
  buffer or host has more events; ack and await the next page, never create another
  bridge reader.
- Apply a page's gap first, then its events in their provided sequence order.
  Sequences are local to `generation`; never compare sequences across generations.
  Message ordinals are one-based within a turn. Text and thinking have independent
  UTF-8 byte offsets, keyed by `(generation,turn,message,kind)`. Write chunks at
  their offsets rather than appending: a duplicate chunk is idempotent. Do not
  splice at character indices or carry offsets into a new generation. Tool partial
  output replaces the displayed bounded tail; it is not another append-only stream.
- `gap.reconcile` is always true. On a gap, discard the live overlay and re-read
  `detail.get` with `DetailRef::Session { id: scope, ... }` before reconciling the
  new overlay. A gap can have `missing:0`: it still means discontinuity (host
  generation change, bridge loss, ending, reconnect, or epoch change).
  `generation:null` means no healthy supported host is currently available.
  A subscriber lagging beyond either buffer bound gets an empty gap page and
  resumes from the present instead of retaining a catch-up queue.
- Re-read that session's transcript detail whenever a turn ends **and whenever a
  tool ends**, because the journal then contains the recorded entries. The live
  overlay is ephemeral, never canonical transcript content. Also discard/re-read
  on a generic `sub.reset`; re-open only if that generic reset closed the topic.
  Session discontinuities themselves use the gap field and keep the topic open.
  Ignore unknown `Other` kinds/states without treating them as durable content.

### Children and agent messages

A connection that requests `session.fleet.v1` receives the watched session's child agents
and their messages with every page (`fleet`, absent when the session has neither). Each
value replaces the previous one; the app never merges them. It clears the fleet when the
watch ends, on a namespace switch, on Forget and on revocation, and shows a held fleet as
stale whenever the session topic is stale. Against a daemon without the capability the
page shows what it showed before, with no children placeholder.

The session page lists the fleet under the run's facts and stages in the run column
(desktop and terminal; a terminal too narrow for that column shows the transcript alone),
and on the phone in a fold under the action bar that opens both lists in a bounded scroll.
The transcript keeps its place. Children read as a published build's slices do: kind when
it is not a slice, title, state, model, current tool, running time, tokens, cost (unknown,
never zero), a review's verdict and an open-question chip. Messages are in time order as
`from → to` with their words and any delivery other than delivered; a shortened list says
`showing 100 of N messages`. Children are read-only: no command addresses them.

### Rust wire definitions

These are the verbatim additions to the existing `Topic` and `SubDelta` enums in
`corral-protocol/src/subscription.rs` (their existing snake-case serde tags remain):

```rust
    Session,
```

```rust
    Session {
        scope: String,
        epoch: u64,
        generation: Option<u32>,
        events: Vec<SessionLiveEvent>,
        cursor: BoardCursor,
        more: bool,
        gap: Option<SessionLiveGap>,
    },
```

The new protocol types are:

```rust
/// Text/thinking offsets are UTF-8 bytes, independent per (generation, turn, message,
/// kind). Applying chunks at their offsets, rather than appending, is idempotent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLiveEvent {
    pub seq: u64,
    pub generation: u32,
    pub at: String,
    #[serde(flatten)]
    pub kind: SessionLiveKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionLiveKind {
    LiveTurn {
        turn: u64,
        #[serde(flatten)]
        state: SessionLiveTurnState,
    },
    LiveText {
        turn: u64,
        message: u64,
        offset: u64,
        text: String,
    },
    LiveThinking {
        turn: u64,
        message: u64,
        offset: u64,
        text: String,
    },
    LiveTool {
        turn: u64,
        call: String,
        tool: String,
        #[serde(flatten)]
        state: SessionLiveToolState,
    },
    LiveQueue {
        steer: u64,
        follow_up: u64,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLiveTurnState {
    Started,
    Ended {
        aborted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLiveToolState {
    Started {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args_truncated: Option<bool>,
    },
    Updated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partial: Option<String>,
    },
    Ended {
        failed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
    },
    #[serde(other)]
    Other,
}

/// Drop the live overlay and reconcile from `detail.get { session }`. A zero
/// `missing` still signals discontinuity (generation change or unavailable host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLiveGap {
    pub after: u64,
    pub resumed: u64,
    pub missing: u64,
    pub reconcile: bool,
}
```

The typed bridge result in ops `agent/bridge/client.rs` mirrors the host page:

```rust
/// A page of ephemeral live events, with a separate cursor from lifecycle events.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LivePage {
    pub generation: u32,
    pub events: Vec<corral_protocol::subscription::SessionLiveEvent>,
    pub last: u64,
    pub next: u64,
    pub more: bool,
    #[serde(default)]
    pub gap: Option<corral_protocol::subscription::SessionLiveGap>,
}
```

