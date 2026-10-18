import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readSubscription, readOnlyStore } from "./credentials.mjs";

const now = 1_800_000_000_000;
/** @param {number} expires */
const access = (expires) => "header." + Buffer.from(JSON.stringify({
  exp: expires / 1000, "https://api.openai.com/auth": { chatgpt_account_id: "fixture-account" },
})).toString("base64url") + ".fixture-signature";

test("reuses only Codex subscription access without changing owner bytes", () => {
  // Given an isolated synthetic owner file with API and refresh material too.
  const dir = mkdtempSync(join(tmpdir(), "pi-credential-test-"));
  const path = join(dir, "auth.json");
  const original = JSON.stringify({ OPENAI_API_KEY: "unused-fixture-key", tokens: {
    access_token: access(now + 3_600_000), refresh_token: "unused-fixture-refresh", account_id: "fixture-account",
  } });
  writeFileSync(path, original, { mode: 0o600 });
  try {
    // When the authorized subscription entry is loaded.
    const loaded = readSubscription(path, "codex", now);
    // Then only access is reusable and the source is unchanged.
    assert.equal(loaded.credential.refresh, "");
    assert.equal(loaded.credential.access, access(now + 3_600_000));
    loaded.verifyUnchanged();
    assert.equal(readFileSync(path, "utf8"), original);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("refuses expired access rather than refreshing an owner login", () => {
  // Given a synthetic expired subscription token.
  const dir = mkdtempSync(join(tmpdir(), "pi-credential-test-"));
  const path = join(dir, "auth.json");
  writeFileSync(path, JSON.stringify({ tokens: { access_token: access(now - 1) } }), { mode: 0o600 });
  try {
    // When reuse is attempted, then it fails before authentication or refresh.
    assert.throws(() => readSubscription(path, "codex", now), { code: "subscription_expired_or_near_expiry" });
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("denies a symlink instead of retargeting the selected owner file", () => {
  // Given a path that redirects to another file.
  const dir = mkdtempSync(join(tmpdir(), "pi-credential-test-"));
  const path = join(dir, "auth.json");
  writeFileSync(join(dir, "other"), "{}");
  symlinkSync(join(dir, "other"), path);
  try {
    // When the selected path is read, then no symlink traversal occurs.
    assert.throws(() => readSubscription(path, "codex", now), { code: "ELOOP" });
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("rejects incompatible API auth without exposing parser input", () => {
  // Given an invalid synthetic auth file whose text is private.
  const dir = mkdtempSync(join(tmpdir(), "pi-credential-test-"));
  const path = join(dir, "auth.json");
  writeFileSync(path, "private-fixture-text", { mode: 0o600 });
  try {
    // When parsed, then the observable failure has only a stable safe code.
    assert.throws(() => readSubscription(path, "codex", now), {
      code: "unsupported_credential_schema", message: "unsupported_credential_schema",
    });
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("blocks refresh callbacks and logout before any credential mutation", async () => {
  // Given an in-memory subscription view, unrelated provider reads stay absent.
  const store = readOnlyStore({ type: "oauth", access: "fixture", refresh: "", expires: Date.now() + 3_600_000, accountId: "fixture" });
  let called = false;
  // When an upstream refresh requests the sole write path.
  await assert.rejects(store.modify("openai-codex", async () => { called = true; }), {
    code: "credential_refresh_or_write_forbidden",
  });
  // Then the callback never runs and logout is denied too.
  assert.equal(called, false);
  assert.equal(await store.read("openai"), undefined);
  await assert.rejects(store.delete("openai-codex"), { code: "credential_logout_forbidden" });
});
