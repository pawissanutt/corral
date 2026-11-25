# API-first model profiles

> **Classic CLI.** This guide documents the classic `corral` CLI and board (v0.4.0), which `install.sh --legacy`
> installs. The current engine is `corral-next` with the Corral app: see the [guide index](../README.md).

API-first profiles start directly on a saved platform API credential. They do not need
subscription quota exhaustion or a previous continuation. API usage is metered separately
from your subscription. Corral cannot tell whether your provider will apply prepaid credits
or charge another payment method.

Saving a key creates a credential, **not a model profile or spending approval**. Selecting a
profile also grants no spending permission. Configure a separate finite approval before the
first paid request. No automatic fallback is needed or enabled by the example below.

## 1. Save the API credential

On the daemon host, use the hidden prompt:

```sh
corral agent login --provider anthropic-api
corral agent login --provider anthropic-api --status
```

Never paste a key into configuration, command arguments, logs or screenshots. For automation,
use `--key-file /path/to/protected-key-file` with an owner-owned regular `0600` file, not a
literal key argument. See [metered API keys](../corral-pi.md#metered-api-keys).

## 2. Configure a separate profile

Add this to the workspace configuration (or your personal profiles). Keep your existing
subscription profile unchanged. Replace the model with an exact supported API catalog model
if necessary; a model supported by a subscription is not necessarily supported by the API.

```toml
[profiles.anthropic-api-sonnet]
harness = "corral-pi"
vendor = "anthropic"
connection = "anthropic-api"
model = "claude-sonnet-4-6"

[profiles.anthropic-api-sonnet.metadata]
display_name = "Sonnet API"
purpose = "Explicit API-first assistant"
guidance = {}
instructions = "Use the explicitly selected API connection."
revision = "1"
evidence = "manually configured"

[profiles.anthropic-api-sonnet.metadata.eligibility]
enabled = true
automatic = false
roles = ["implementation"]
projects = []

[[profiles.anthropic-api-sonnet.metadata.candidates]]
id = "sonnet-api"
harness = "corral-pi"
connection = "anthropic-api"
provider = "anthropic"
model = "claude-sonnet-4-6"
args = []
evidence = "manually configured"
cost = { kind = "metered", currency = "USD", maximum_micros = 1000000 }

# Explicit spending opt-in: uncomment these four lines only after choosing a budget.
# [profiles.anthropic-api-sonnet.metadata.api_start]
# roles = ["assistant"]
# approval = "owner-approved-api-assistant-5-usd"
# spending = { currency = "USD", maximum_micros = 5000000 }
```

The preferred candidate must exactly match the profile's connection, model and effort. If you
set an effort, set it identically on both. `connection` is the billing/auth identity, not just
a display label: `anthropic-api` uses only its saved API key; an omitted connection on an
Anthropic profile retains the subscription route. OpenAI uses `connection = "openai-api"`
with `vendor`/candidate `provider = "openai"` and an exact supported API model.

## 3. Explicitly approve a finite allowance

The profile above can be listed but cannot make paid requests. Only after deciding your
budget, explicitly uncomment the four `metadata.api_start` lines in that complete profile
example (the table header, roles, approval and spending). They are commented out by default
so copying the example does not approve spending.

This example explicitly approves **up to USD 5 per assistant session lineage**, not USD 5
across all sessions. One USD is 1,000,000 micros. Choose your own finite positive allowance;
the approval identifier must be nonempty. Role names are `assistant`, `pm`, `worker`,
`reviewer`, `planner` and `plan_reviewer`; list only roles you intend to authorize.
`api_start` is independent of `metadata.fallback`: do not enable automatic fallback merely
to use an API-first profile.

**Spending caps are currently disabled.** `metadata.api_start` still names the approved roles,
route and approval identifier, but its `spending` allowance is not enforced: requests are never
refused for exceeding it, sessions never stop as exhausted, and editing the approval or
allowance does not block resuming an existing paid session. Consumed and reserved usage is
still recorded for usage reporting. Remove `api_start` to withdraw future API-first permission;
in-flight requests cannot be recalled.

## 4. Select the profile
For a terminal session, select the profile for one launch without changing defaults:

```sh
corral pi --assistant --profile anthropic-api-sonnet
corral pi --pm --profile anthropic-api-sonnet
corral pi --build --profile anthropic-api-sonnet
```

The profile must approve the corresponding role; Build uses the `assistant` spending
approval. `--profile` cannot be combined with `--model`, `--provider` or `--effort`: an API
approval names the exact preferred connection, model and effort.

Plain `corral pi` also offers an optional **Profile** choice. Leave it at **default** to
retain each mode's existing default. Inside a supervised terminal session, `/profiles`
opens the profile picker, `/profiles <name>` selects an exact name, and **Alt+R** cycles
the available profiles. Billing labels distinguish subscription and metered API routes.
Switching waits for idle work. Bound profile changes ask before opening a new conversation;
an assistant's previous session stays resumable. The confirmation names any new session's
own spending allowance. Build changes retain the worktree and transcript, but an established
paid build or PM cannot change its frozen route or replenish its allowance.


In corral-app, open the composer's model selector and select `anthropic-api-sonnet`.
It is labelled **Anthropic · API (metered)**; subscription profiles are labelled
**Anthropic · subscription**, even for the same model. Settings › Assistant lists the same
profiles. To make this the default for new assistants:

```sh
corral config set assistant.agent anthropic-api-sonnet
```

The daemon host needs the credential and compatible installed corral-pi assets. Session
facts show the host's recorded connection, not whatever a profile may have become later.
A retained session cannot silently change billing routes; use a new session when a frozen
binding prevents a switch. A refused selection leaves the existing route unchanged.

API-first admission applies to canonically bound native roles, including daemon assistants,
and workspace Build sessions. Build keeps its ordinary worktree, off-board tools and exit
workflow; its workspace ledger tracks metered parent, child and compaction requests without
enforcing an allowance. Resume fences the previous host and retains consumed and
uncertain reservations. See [Build sessions](../build-sessions.md#named-profiles-and-metered-apis)
for preset routing. Bare sessions (`corral pi --bare --profile <api-profile>`) also accept
API profiles; they have no corral ledger, so their API usage is not recorded. Non-workspace
`--build` sessions still refuse API profiles.
Paid summary profiles remain separately restricted.

## Refusals and recovery

- **Missing API credential:** run `corral agent login --provider anthropic-api` on the daemon
  host. A subscription login does not satisfy this requirement.
- **Unsupported model/effort:** choose the exact API catalog identity and a supported effort;
  update both profile and preferred candidate. No subscription or model fallback is inferred.
- **Spending approval required:** explicitly configure `metadata.api_start` with the role,
  approval identifier and finite USD allowance. Saving a credential or selecting the model
  cannot fix this refusal by itself.
- **Frozen route changed:** a paid session resumes only on its original profile and exact
  API route (connection, model, effort). Approval or allowance edits do not block resume.

Existing subscription profiles and [quota-triggered paid continuation](../corral-pi.md#paid-continuation-after-subscription-quota)
keep their existing configuration and behavior. This guide does not authorize any live
configuration changes or paid verification requests.
