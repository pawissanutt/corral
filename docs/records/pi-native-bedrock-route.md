# Pi-native Amazon Bedrock route

## Decision

On 2026-10-29 the owner approved Amazon Bedrock as an explicit, metered exception to
OD5's subscription-only pi-native rule. A profile must explicitly set
`vendor = "amazon-bedrock"` and have a `corral agent login` binding. There is no separate
metered profile field; automatic Bedrock availability candidates must declare metered cost.
Traffic is billed per token to the owner's AWS account, with one notice per role launch before
any paid call and no per-run spending cap.

The owner restricted credentials to a named AWS profile: static keys, `source_profile`
assume-role chains, or IAM Identity Center SSO. corral reads the normal home AWS files, strips
ambient `AWS_*` variables, and refuses unsupported mechanisms such as `credential_process`.
SSO is read-only: corral reads the cached token and calls GetRoleCredentials, never OIDC token
refresh or login and never writes `~/.aws`. The user runs `aws sso login` when needed. Each
cached credential layer attempts renewal near expiry but retains unexpired credentials if
renewal fails; hard expiry is the boundary for refusal.

Routing vendor and model family are distinct. Bedrock selects the provider, guard and pricing;
its model publisher determines reviewer independence. Thus Bedrock Claude and Anthropic Claude
are the same family, and an unknown Bedrock family fails closed for cross-family review.

## Guard and verification boundary

The host patches Smithy Node HTTP handlers to delegate to the fetch handler and the guarded
`fetch`, which checks method and URL and requires SigV4 for the bound model's regional
ConverseStream endpoint. STS is permitted only for role chains and the SSO portal only for SSO;
OIDC and all other destinations are refused. A Node TCP socket and HTTP client tripwire provides
defense in depth while permitting Unix sockets. This method-and-path guard was chosen over a
socket allowlist, which cannot enforce request paths.

Offline fixture tests cover the route and credential behavior. Live AWS verification is not part
of CI or the corral gate: it is deferred to a user with Bedrock access via
`scripts/bedrock-smoke.sh`, which requires explicit `--yes-metered` before sending. See the
[pi-native guide](../guide/pi-native.md#amazon-bedrock-metered-opt-in) for setup and limits.
