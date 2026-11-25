# Automating board work

In **Settings → Configuration → Automations**, create an agent with a Pi profile and instructions, then add a rule that names it. Choose **Task blocked** to react to a new block, or **Cron (UTC)** to run on a schedule. Enable the rule and save the draft. Rules are workspace-wide and off by default. Preview edits a sample file; it never starts an agent.

For example, use one agent to triage blockers as they happen and review the board each weekday. The two rules use separate named chats and do not consume coding slots. Their saved entries in `.corral-next/corral.toml` look like this:

```toml
[automation.agents.triage]
profile = "pi"
instructions = "For a blocked task, read its history and comment with the next concrete action. For a scheduled review, file missing follow-ups after checking for duplicates."

[automation.rules.on-block]
agent = "triage"
event = "blocked"
enabled = true

[automation.rules.weekday-review]
agent = "triage"
cron = "0 9 * * 1-5"
enabled = true
```

**Authority.** Saving an enabled rule authorizes its agent to act on the board under the interactive assistant's usual permissions. Be specific about what it should do. The host binds the saved instructions to automated turns for that rule; a block reason is data, not a new instruction or approval. Normal refusals, owner questions and approval checks still apply.

**Timing.** Cron has five numeric fields: minute, hour, day of month, month, weekday. Use `*`, comma lists, ranges or steps; Sunday is `0` or `7`. If both day fields are restricted, either can match. The example fires at 09:00 UTC Monday through Friday. Missed cron minutes are not replayed. A blocked rule uses committed block events, not the current list of blocked tasks. Creating, enabling or editing a rule starts it from the current board and minute, not from history. Use the app's Save to make this boundary precise. For hand edits, the daemon uses the board head when it observes the changed file; it cannot detect a pause and re-enable made entirely between observations.

**Delivery.** Open the rule's chat from its last result in Settings. `sent` means the host accepted the input, not that the agent completed the work. Busy, paused or waiting chats skip inputs instead of queuing them. Each rule can try once per UTC minute; additional matches are skipped. If a submission was interrupted and the daemon cannot tell whether it arrived, it reports `delivery unknown` and does not retry. Inspect the chat and board before taking action yourself.

**Stopping.** Disable the rule and save to stop future firings; this does not cancel work already underway. Terminating a chat keeps it stopped until owner intervention. End a chat to let the next firing start a replacement; if its policy changed while manually stopped, end the old chat rather than relaunching it with stale instructions. Remove or reassign a rule before deleting its agent.
