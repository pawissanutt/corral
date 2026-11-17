# API-first model profiles

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
literal key argument. See [metered API keys](corral-pi.md#metered-api-keys).

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

The canonical paid episode owns the allowance. Each physical paid request reserves a
conservative priced bound **before** sending; the request is refused if it will not fit.
Uncertain requests remain reserved. Recovering a host or resuming the same paid lineage does
not reset the allowance or silently switch to subscription. Changing policy requires renewed
approval and does not replenish an existing episode. Remove `api_start` to withdraw future
API-first permission; in-flight requests cannot be recalled.

## 4. Select the profile

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

API-first admission applies to canonically bound native roles, including daemon assistants.
Untethered `corral pi` build sessions and bare sessions have no canonical paid-role ledger and
remain refused rather than reinterpreting an API profile as subscription billing. Use a
daemon assistant for API-first chat. Paid summary profiles remain separately restricted.

## Refusals and recovery

- **Missing API credential:** run `corral agent login --provider anthropic-api` on the daemon
  host. A subscription login does not satisfy this requirement.
- **Unsupported model/effort:** choose the exact API catalog identity and a supported effort;
  update both profile and preferred candidate. No subscription or model fallback is inferred.
- **Spending approval required:** explicitly configure `metadata.api_start` with the role,
  approval identifier and finite USD allowance. Saving a credential or selecting the model
  cannot fix this refusal by itself.
- **Allowance exhausted or insufficient:** no paid request is sent. Review consumed and
  reserved usage; do not retry to bypass the cap. A fresh independently approved session is
  a new allowance, not a refund of uncertain requests.
- **Frozen binding/policy changed:** retain the exact approved route or start a new explicitly
  approved session. Corral does not silently reinterpret a paid session as a subscription.

Existing subscription profiles and [quota-triggered paid continuation](corral-pi.md#paid-continuation-after-subscription-quota)
keep their existing configuration and behavior. This guide does not authorize any live
configuration changes or paid verification requests.
