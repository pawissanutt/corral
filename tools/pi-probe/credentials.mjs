import { constants, openSync, fstatSync, readSync, closeSync } from "node:fs";
import { createHash } from "node:crypto";

export class ProbeFailure extends Error {
  /**
   * `origin` separates a probe-harness failure (the default) from an observed
   * upstream behavior that differs from the T-136 contract.
   * @param {string} code @param {"harness_port" | "upstream_incompatibility"} [origin]
   */
  constructor(code, origin = "harness_port") {
    super(code);
    this.code = code;
    this.origin = origin;
  }
}

/** @param {unknown} value @returns {value is Record<string, unknown>} */
export function record(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** @param {string} text @returns {unknown} */
function privateJson(text) {
  try {
    return JSON.parse(text);
  } catch {
    // Parser diagnostics can include secret input. Never propagate them.
    throw new ProbeFailure("unsupported_credential_schema");
  }
}

/** @param {number} fd */
function privateBytes(fd) {
  const buffer = Buffer.alloc(65_537);
  const count = readSync(fd, buffer, 0, buffer.length, 0);
  if (count > 65_536) throw new ProbeFailure("credential_file_not_owner_regular_bounded");
  return buffer.subarray(0, count);
}

/** @typedef {{type: "oauth", access: string, refresh: string, expires: number, accountId: string}} SubscriptionCredential */

/**
 * Read one authorized file, retain only the selected subscription access token.
 * Never retain or refresh its refresh token, write the file, or change permissions.
 * @param {string} path
 * @param {"codex" | "pi"} source
 * @param {number} nowMs
 */
export function readSubscription(path, source, nowMs) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  let bytes;
  let stat;
  try {
    stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > 65_536 || stat.uid !== process.getuid?.()) {
      throw new ProbeFailure("credential_file_not_owner_regular_bounded");
    }
    bytes = privateBytes(fd);
  } finally {
    closeSync(fd);
  }
  const digest = createHash("sha256").update(bytes).digest("hex");
  const data = privateJson(bytes.toString("utf8"));
  if (!record(data)) throw new ProbeFailure("unsupported_credential_schema");
  if (source === "codex" && data.auth_mode !== undefined && data.auth_mode !== "chatgpt") {
    throw new ProbeFailure("subscription_not_selected");
  }
  const entry = source === "codex" ? data.tokens : data["openai-codex"] ?? data["chatgpt-subscription"];
  if (!record(entry)) throw new ProbeFailure("subscription_credential_missing");
  const access = source === "codex" ? entry.access_token : entry.access;
  if (source === "pi" && entry.type !== "oauth") throw new ProbeFailure("subscription_credential_missing");
  if (typeof access !== "string" || access.split(".").length !== 3) {
    throw new ProbeFailure("subscription_access_token_missing");
  }
  const payload = access.split(".")[1];
  if (!payload) throw new ProbeFailure("unsupported_credential_schema");
  const claims = privateJson(Buffer.from(payload, "base64url").toString("utf8"));
  if (!record(claims) || typeof claims.exp !== "number" || !Number.isFinite(claims.exp)) {
    throw new ProbeFailure("subscription_expiry_missing");
  }
  const auth = claims["https://api.openai.com/auth"];
  if (!record(auth) || typeof auth.chatgpt_account_id !== "string" || !auth.chatgpt_account_id) {
    throw new ProbeFailure("subscription_account_missing");
  }
  const expires = claims.exp * 1000;
  if (expires <= nowMs + 360_000) throw new ProbeFailure("subscription_expired_or_near_expiry");
  if (source === "codex" && entry.account_id != null && entry.account_id !== auth.chatgpt_account_id) {
    throw new ProbeFailure("subscription_account_mismatch");
  }
  /** @type {SubscriptionCredential} */
  const credential = { type: "oauth", access, refresh: "", expires, accountId: auth.chatgpt_account_id };
  return {
    credential,
    source,
    mode: stat.mode & 0o777,
    verifyUnchanged() {
      const current = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
      try {
        const after = fstatSync(current);
        if (after.size > 65_536 || after.ino !== stat.ino || after.mode !== stat.mode ||
            createHash("sha256").update(privateBytes(current)).digest("hex") !== digest) {
          throw new ProbeFailure("owner_credential_changed_during_probe");
        }
      } finally {
        closeSync(current);
      }
    },
  };
}

/**
 * @param {SubscriptionCredential} credential
 * @returns {import("@earendil-works/pi-ai").CredentialStore & {readonly reads: number}}
 */
export function readOnlyStore(credential) {
  let reads = 0;
  return {
    get reads() { return reads; },
    /** @param {string} provider */
    async read(provider) {
      if (provider !== "openai-codex") return undefined;
      if (credential.expires <= Date.now() + 300_000) {
        throw new ProbeFailure("subscription_expired_or_near_expiry");
      }
      reads++;
      return { ...credential };
    },
    async list() {
      return [{ providerId: "openai-codex", type: /** @type {const} */ ("oauth") }];
    },
    async modify() { throw new ProbeFailure("credential_refresh_or_write_forbidden"); },
    async delete() { throw new ProbeFailure("credential_logout_forbidden"); },
  };
}
