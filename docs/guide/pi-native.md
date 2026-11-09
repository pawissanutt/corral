# corral-pi guide

The harness guide has moved to [corral-pi](corral-pi.md). This compatibility page
keeps historical record links usable.

<a id="herdr-status"></a>
See [herdr status](corral-pi.md#herdr-status).

<a id="amazon-bedrock-metered-opt-in"></a>
See [Amazon Bedrock](corral-pi.md#amazon-bedrock-metered-opt-in).

## Annotation host API

The Changes panel can use `src/host/annotations.ts` without exposing an annotation
mutation tool to the model. This API does not render a panel.

- `read_annotations` is an authenticated, receipt-free lookup. Worker and reviewer
  hosts may read only their bound task; PM hosts stay within their effective project
  scope; assistant hosts may read any task in their store. Planner roles are refused
  with `role_not_permitted`. The payload is `{task, thread?, query?}`; query accepts
  state/anchor filters, a cursor and a limit (1–64). List pages contain summaries;
  thread pages contain entries. Continue with `next_cursor`, retaining the same query.
- `readAnnotations(client, query)` validates AnnotationsView v1 and distinguishes an
  unavailable candidate from an available empty list. Each encoded response, including
  the transport envelope and cursor, is bounded to 32 KiB. Candidate paths may be
  explicitly truncated; entries are never truncated.
- `submitOwnerAnnotation(client, action, requestKey)` sends the host-only event kind
  `owner_annotation`, with `{kind, task, request_key, action}`. The nested action is
  tagged open/reply/resolve/reopen. Open requires the observed `expected_run`, base,
  head, path, side and text (optionally range); other actions require thread and
  `expected_revision`. No payload may supply an author or other unknown fields.
- The owner client wraps the host's Corral client and supplies `nextEventRequestId()`
  from its EventLog via `eventRequestId(generation, event.seq)`. Invoke it only for an
  explicit owner panel action. Never put this client, callback or its credentials in
  a role loadout, model tool, Code Mode table or shell payload. Tool request IDs and
  Code Mode parents cannot submit this event.
- Worker, reviewer, PM and assistant hosts can submit within the same scopes as reads.
  A live, unfenced generation is required, rechecked in the annotation transaction
  **before replay**. The stored author is owner via pi_panel, with authenticated
  container, session and generation—not the pane's agent role.
- Create one `newAnnotationRequestKey()` per owner action and retain it with the exact
  action until a terminal result. Committed and replayed results include task, thread,
  entry, revision and state; refusals preserve their code/detail. Timeouts, transport
  errors and malformed output are **unknown**, never committed. Retry unknown with
  the same key and payload after fresh authorization, even in a new host generation.
  Canonical annotation entries own retry identity; neither reads nor these events
  append a separate agent receipt. Mutation responses are at most 8 KiB.

Annotation refusals use `annotation_refused` with `detail.code` and typed error detail;
existing authentication, `out_of_scope`, `bad_request` and `agent_fenced` refusals remain
unchanged. Candidate anchors never relocate: commits make older notes stale, newer
runs supersede them, and missing worktrees report unavailable. Annotations are review
context, not evidence; they change no prompt, gate, review verdict or acceptance rule.
