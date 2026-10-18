# Task comments

Comments are an append-only thread on a task, separate from its specification. Leave notes,
decisions, review remarks and links at any status, including Done or Dropped:

```sh
corral comment T-046 "Keep the public API stable."
corral comment T-046 "Owner confirmed the scope." --author pm
corral show T-046
corral show T-046 --json
```

The default author is `owner`; `--author pm` attributes a PM note without impersonating the
owner. Each entry records its author, timestamp and exact text. `show` lists the thread,
and `show --json` always includes a `comments` array.

Comments are stored in the task file's YAML frontmatter and logged as `commented` events.
They do not change the Markdown spec body, spec revision, candidate verification identity,
or the compatibility of existing gates, reviews and portable checkpoints. A scope change
still belongs in the task specification rather than only in a comment.

An owner comment using `owner-ok: <action> <reason>` also records explicit permission
for one protected idea change. Arbitrary prose never grants permission; see
[sprint review and owner ideas](sprint-review.md).

Dispatch and reviewer prompts include the thread under **Owner comments**, with the task
file as its durable source. This also gives the review autopilot's reviewer the context.
Custom prompt templates receive the comments even when they have no comment placeholder.

## Delivering a note

```sh
corral comment T-046 "Use the existing reply path." --send
```

`--send` requires a current running agent (Running or Needs me). It saves the comment first,
then sends an owner note referencing the task file through the supported reply delivery path.
A stuck agent queues it. A failed delivery keeps the saved comment and, once accepted by
the reply path, its exact pending message; use `corral reply T-046 --resend` to retry that
message without adding another comment. An already pending, different reply must be
resolved first. Without `--send`, comments never contact an agent.

For an OmO coding or planner run with a native question, `corral reply` binds the complete message to
that session's full request ID and question/option identity before sending it through OmO's
authenticated terminal control endpoint. The reply is a native comment on the question, not
a guessed menu selection or a paste into its own-answer editor. Collapsed and multiline drafts
do not require Enter keys. Ordinary replies retain their existing composer verification.

An uncertain native acknowledgement leaves the full reply pending. `--resend` checks the
bound request's settlement and matching answer, or retries only that same request; it never
falls back to the ordinary composer or answers a replacement question. Native acceptance is
logged as `via=omo_question`; a reconciled answer has `submitted=true, attempts=0` because
no new submission was attempted. Missing endpoint support, ambiguous questions or a changed
identity leave a manual-action error and the pending answer intact. OmO's native comment
format trims surrounding whitespace for model display; corral retains and sends the original
message unchanged.

Opaque collapsed ordinary-composer pastes also remain pending: corral recognizes both
`[Pasted text ...]` and OmO's `[paste #...]` markers and refuses another paste or Enter.
Clearing an editor does not clear the canonical pending reply, and a new, different reply
cannot replace it. No reply replacement or cancellation command is introduced.

## Terminal interface

Open a task's detail and select **Comments**. The thread shows author, time and text.
The tab's add-comment action opens an input; its visible key hints and clickable controls
support keyboard-only and mouse use. The new note is saved to the same thread as CLI notes.

Editing, deleting, agent attribution, and comments attached to individual artifacts or runs
are not supported.
