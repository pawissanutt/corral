# Pane delivery

`corral-herdr` exports:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryVia {
    AgentPrompt,
    SendText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub via: DeliveryVia,
    pub submitted: bool,
    pub attempts: u32,
    pub error: Option<String>,
}
```

The public signatures are:

```rust
pub fn agent_prompt(&self, pane_id: &str, text: &str) -> anyhow::Result<Delivery>;
pub fn pane_send_text(
    &self,
    pane_id: &str,
    text: &str,
    submit: bool,
) -> anyhow::Result<Delivery>;
```

Both return `Ok(Delivery)` for transport and verification failures, including spawn
errors. `Ok` alone does not mean submission. Callers must retain pending text unless
`submitted` is true. `error` carries failure context; no failure clears the composer.
All herdr subprocesses use argv and null stdin.

## Native prompt and resend

Before `agent prompt`, an input snapshot checks for the same pending text. If found,
delivery goes directly to verified Enter without invoking a native transport that
could paste a duplicate. Failure of this optional preflight does not prevent a native
prompt: the semantic transport does not require terminal reads.

A successful native `agent prompt` reports `agent_prompt`, `submitted: true`,
`attempts: 1`, `error: null`. A native failure other than no-agent/not-ready/blocked
reports `agent_prompt`, `submitted: false`, `attempts: 1`, with the error. It does not
fall back on arbitrary errors. Native success is the semantic backend's
acknowledgment of submission, not evidence that the agent completed the work.

No-agent/not-ready/blocked failures use the terminal path. That path always reads
again before pasting, including after a native attempt that may already have put text
in the composer. Matching input is reused. It never pastes again during Enter retries.

## Bounded terminal verification

1. Read 200 recent unwrapped lines before writing. A read failure aborts terminal
   delivery without pasting or pressing Enter.
2. If the full normalized text is already in the composer, reuse it. Otherwise paste
   once and read immediately, then up to nine more times at 100 ms intervals.
3. If the text never appears, fail with `submitted: false`, `attempts: 0`. No Enter is
   sent. Empty or whitespace-only text likewise cannot be verified.
4. For `submit: false`, stop after observing the text, with `submitted: false`,
   `attempts: 0`, `error: null`.
5. For submission, send Enter and poll immediately plus up to nine more reads at
   100 ms intervals for the text to disappear from the input region.
6. If it remains, retry Enter twice, with 100 ms then 200 ms backoff. Re-read after
   backoff before sending another Enter: if it already disappeared, stop successfully
   without sending Enter into a fresh composer.
7. If all three Enter attempts fail verification, report `submitted: false` and an
   error. `attempts` counts Enter calls, including failed calls, not reads or pastes.
   When fallback follows a native refusal, it counts only the fallback's Enter calls.

The maximum scheduled wait is 3.9 seconds, plus subprocess runtime. Poll counts and
Enter counts are bounded, but this is not a subprocess timeout. Any mandatory read,
paste, or key transport failure returns failure metadata immediately, with the Enter
count reached so far. An uncertain key transport failure is not asserted successful.

Production uses the same internal poller exercised by tests, defaulting to
`std::thread::sleep` as its wait function. Both public delivery APIs use the wait
configured by `Herdr::with_poll_wait(self, wait: fn(Duration)) -> Self`.
Tests can use `Herdr::with_bin(bin, None).with_poll_wait(|_| {})`, including ops
tests asserting that a never-visible paste retains pending state, without real
waits. Low-level tests also inject recording waits; fake-herdr command transitions
drive visibility and acceptance deterministically. No delivery test depends on
elapsed time or a lucky scheduling interval.

## Pending replies and integration

`corral reply ID --resend` retries the stored text. Pending change requests in
Review are reopened through `request_changes`. `pending_reply` is cleared only
after explicit successful submission, never merely because sync reports an agent
as working. The board displays the pending badge on the parent and latest review
child.

A failed PM push leaves its event cursor uncommitted so the next sync retries it.
A reviewer PM delivery failure is retained in the `review_runs` child; retry it
with `corral review-run ID`. All these callers must inspect `Delivery::submitted`;
`Ok(Delivery)` with an error is still a failed submission. Terminal `send_text`
attempt counts include only Enter calls, not the preceding native refusal, paste,
or visibility polls.

## Input heuristic and limits

The last line beginning with `❯` (OmO/omp) or `>` (Claude), after leading whitespace,
starts the input region. The marker must be followed by whitespace or end of line.
An empty final marker counts: text in earlier transcript prompts must not trigger a
duplicate-detection match or prevent successful submission verification.

The input region runs from that marker to the bottom of the snapshot. Whitespace is
collapsed in both input and requested text before matching the entire normalized
request as a substring. This supports multiline input, CRLF, and pastes displayed
with collapsed whitespace. It rejects partial visibility of a multiline request.
Recent-unwrapped reads avoid ordinary soft-wrap splits.

Without a recognized marker, only the bottom nonempty line is inspected. This is a
best-effort shell/unknown-agent heuristic, not an application acknowledgment.
Disappearance after Enter is evidence that input left the composer, not proof the
agent understood it or completed the work.

Opaque summaries such as `[Pasted text #1 +N lines]` do not prove the full text is
present; they cannot produce verified success. A recognized `[Pasted text` summary
in the initial composer refuses both a native prompt and another paste, leaving the
reply pending rather than risking duplication. ANSI-decorated markers, alternative
prompt layouts, composer content exceeding 200 lines, or footer/transcript text that
resembles input can defeat the heuristic. The read API provides no authoritative
composer boundary or paste identity. Consequently duplicate avoidance applies when
the actual pending text is visible or a recognized opaque summary blocks another
paste. Other summary formats cannot establish identity. A redraw or manual deletion can also look like submission.
There is no exactly-once guarantee across ambiguous transport failures or resending
after text has already left the composer.

The fake-herdr tests exercise lost first Enter, exhaustion, never-visible paste,
delayed multiline visibility, last-marker selection, paste-only delivery, native
success/error metadata, read/paste/key failures, resend before native prompt, and a
native refusal that already pasted. They do not touch a live herdr session.
