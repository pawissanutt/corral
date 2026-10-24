// Fixed limits of the claude-route-probe (T-306, OD5). The owner budget recorded on T-306 is
// at most 20 requests and 50K output tokens, $0 API (subscription only). The probe plans
// far fewer; these constants are the hard ceiling the guard enforces before any dispatch.
export const ANTHROPIC_ORIGIN = "https://api.anthropic.com";
export const MESSAGES_PATH = "/v1/messages";
export const PROVIDER = "anthropic";
export const DEFAULT_MODEL = "claude-haiku-5-5";

export const BUDGET = Object.freeze({ requests: 20, outputTokens: 50_000, apiSpendUsd: 0 });
/** Cap on `max_tokens` of any single request; reserved against the output budget before dispatch. */
export const MAX_TOKENS_PER_REQUEST = 256;
/** Requests one probe process may dispatch: a plain reply and one tool call. */
export const PROBE_REQUESTS = 2;

/** Subscription OAuth access tokens carry this marker; API keys (`sk-ant-api`) never do. */
export const OAUTH_TOKEN_MARKER = "sk-ant-oat";

/** Directory names the probe must never read from or write to (other harnesses' credential stores). */
export const FORBIDDEN_STORE_SEGMENTS = Object.freeze([".claude", ".codex", ".omo", ".pi", ".opencode"]);
