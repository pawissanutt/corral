// Fixture auth module: a synthetic credential that only the fake provider transport accepts.
// It is copied to src/auth/index.ts only inside a temp asset directory carrying fixture.json.
import type { AuthModule } from "../../../src/auth/types.ts";
import { type FakeTransport, fakeCodexTransport } from "./fake-transport.ts";

const KEY = Symbol.for("corral-pi.fixture.transport");

function segment(value: Record<string, unknown>): string {
  for (let nonce = 0; ; nonce++) {
    const text = Buffer.from(JSON.stringify({ ...value, jti: `synthetic-${nonce}` })).toString("base64").replace(/=+$/, "");
    if (!/[+/]/.test(text)) return text;
  }
}

function syntheticAccess(nowMs: number): string {
  return [
    segment({ alg: "none", typ: "JWT" }),
    segment({ exp: Math.floor(nowMs / 1000) + 3600, "https://api.openai.com/auth": { chatgpt_account_id: "synthetic-account" } }),
    "synthetic-signature",
  ].join(".");
}

/** The fake transport of this process, shared with the fixture role's control commands. */
export function fixtureTransport(): FakeTransport {
  const holder = globalThis as unknown as Record<symbol, FakeTransport | undefined>;
  holder[KEY] ??= fakeCodexTransport();
  return holder[KEY];
}

let reads = 0;

const auth: AuthModule = {
  provider: "openai-codex",
  credentials() {
    const access = syntheticAccess(Date.now());
    return {
      async read(provider: string) {
        if (provider !== "openai-codex") return undefined;
        reads++;
        return { type: "oauth" as const, access, refresh: "", expires: Date.now() + 3_600_000, accountId: "synthetic-account" };
      },
      async list() {
        return [{ providerId: "openai-codex", type: "oauth" as const }];
      },
      async modify() {
        throw new Error("fixture credentials are read-only");
      },
      async delete() {
        throw new Error("fixture credentials are read-only");
      },
    };
  },
  transport() {
    return fixtureTransport().send;
  },
  state() {
    const transport = fixtureTransport();
    return { fixture: true, credential: "synthetic", reads, transport_requests: transport.requests,
      transport_held: transport.held };
  },
};

export default auth;
