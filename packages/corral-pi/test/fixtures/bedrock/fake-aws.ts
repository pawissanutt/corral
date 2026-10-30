// Offline Bedrock fixtures (T-504): a temporary HOME/.aws, a fake AWS transport behind the host's
// guarded fetch (Bedrock ConverseStream as a canned event stream, STS AssumeRole XML, SSO
// GetRoleCredentials JSON), a byte-and-mtime snapshot of HOME/.aws, and a Bedrock turn wired
// exactly as src/host/main.ts wires a Bedrock host. Nothing here opens a socket.
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";

import type { AssistantMessage } from "@earendil-works/pi-ai";
import { EventStreamCodec } from "@smithy/core/event-streams";

import { type AwsBinding, bedrockRoute, createAwsCredentialResolver } from "../../../src/auth/aws.ts";
import { hostRules } from "../../../src/auth/providers.ts";
import auth from "../../../src/auth/index.ts";
import { createModelCatalog } from "../../../src/host/catalog.ts";
import { type GuardedFetch, guardedFetch } from "../../../src/host/restrict.ts";
import { adoptSmithyHandlers } from "../../../src/host/smithy.ts";
import { authContext, restrictProvider } from "../../../src/host/session.ts";

export const MODEL = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";
export const REGION = "us-west-2";
export const CONVERSE_URL = `https://bedrock-runtime.${REGION}.amazonaws.com/model/${encodeURIComponent(MODEL)}/converse-stream`;
export const FIXTURE_KEY = "AKIAFIXTUREKEY000001";
export const BINDING: AwsBinding = { profile: "dev", region: REGION };

export function awsHome(files: Record<string, string>): string {
  const home = mkdtempSync(join(tmpdir(), "corral-bedrock-"));
  for (const [path, text] of Object.entries(files)) {
    const file = join(home, ".aws", path);
    mkdirSync(join(file, ".."), { recursive: true });
    writeFileSync(file, text);
  }
  return home;
}

export const STATIC_DEV = { credentials: `[dev]\naws_access_key_id = ${FIXTURE_KEY}\naws_secret_access_key = fixture-secret\n` };

export function ssoCacheFile(key: string): string {
  return `sso/cache/${createHash("sha1").update(key).digest("hex")}.json`;
}

/** A cached SSO token with the refresh fields the AWS SDK would use to refresh and rewrite it. */
export function ssoToken(expiresAt: number, startUrl = "https://corp.awsapps.com/start"): string {
  return JSON.stringify({
    startUrl, region: "us-east-1", accessToken: "fixture-sso-access-token", expiresAt: new Date(expiresAt).toISOString(),
    refreshToken: "fixture-refresh-token", clientId: "fixture-client-id", clientSecret: "fixture-client-secret",
    registrationExpiresAt: new Date(expiresAt + 86_400_000).toISOString(),
  });
}

export const SSO_SESSION_CONFIG = "[profile dev]\nsso_session = corp\nsso_account_id = 111122223333\nsso_role_name = Dev\n\n" +
  "[sso-session corp]\nsso_start_url = https://corp.awsapps.com/start\nsso_region = us-east-1\nsso_registration_scopes = sso:account:access\n";
export const LEGACY_SSO_CONFIG = "[profile dev]\nsso_start_url = https://legacy.awsapps.com/start\nsso_region = us-east-1\n" +
  "sso_account_id = 111122223333\nsso_role_name = Dev\n";

export interface AwsSnapshot {
  readonly [path: string]: { readonly sha256: string; readonly mtimeMs: number } | "dir";
}

export function snapshotAws(home: string): AwsSnapshot {
  const out: Record<string, AwsSnapshot[string]> = {};
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      const stat = statSync(path);
      if (stat.isDirectory()) {
        out[relative(home, path)] = "dir";
        walk(path);
      } else {
        out[relative(home, path)] = { sha256: createHash("sha256").update(readFileSync(path)).digest("hex"), mtimeMs: stat.mtimeMs };
      }
    }
  };
  walk(join(home, ".aws"));
  return out;
}

export interface SentRequest {
  readonly method: string;
  readonly url: string;
  readonly authorization: string;
  readonly securityToken: string | null;
  readonly headers: Record<string, string>;
}

export interface IssuedKeys {
  readonly accessKeyId: string;
  readonly expiration: number;
}

const codec = new EventStreamCodec((bytes) => new TextDecoder().decode(bytes), (text) => new TextEncoder().encode(text));

function frame(type: string, body: unknown): Uint8Array {
  return codec.encode({
    headers: {
      ":event-type": { type: "string", value: type },
      ":message-type": { type: "string", value: "event" },
      ":content-type": { type: "string", value: "application/json" },
    },
    body: new TextEncoder().encode(JSON.stringify(body)),
  });
}

/** A ConverseStream reply: messageStart, one "OK" text delta, messageStop and usage metadata. */
export function converseBody(text = "OK"): Blob {
  return new Blob([
    frame("messageStart", { role: "assistant" }),
    frame("contentBlockDelta", { contentBlockIndex: 0, delta: { text } }),
    frame("contentBlockStop", { contentBlockIndex: 0 }),
    frame("messageStop", { stopReason: "end_turn" }),
    frame("metadata", { usage: { inputTokens: 11, outputTokens: 2, totalTokens: 13 }, metrics: { latencyMs: 1 } }),
  ] as ConstructorParameters<typeof Blob>[0]);
}

export interface FakeAws {
  readonly send: (input: string | URL | Request, init?: RequestInit) => Promise<Response>;
  readonly requests: SentRequest[];
  /** Queued STS AssumeRole answers: issued keys, or "fail" for a 403 AccessDenied. */
  readonly sts: (IssuedKeys | "fail")[];
  /** Queued SSO GetRoleCredentials answers. */
  readonly portal: IssuedKeys[];
  bedrock: "ok" | "denied";
}

export function fakeAws(): FakeAws {
  const fake: FakeAws = {
    requests: [], sts: [], portal: [], bedrock: "ok",
    async send(input, init) {
      const request = input instanceof Request ? input : new Request(input, init);
      const headers = Object.fromEntries(request.headers.entries());
      fake.requests.push({ method: request.method, url: request.url, authorization: headers.authorization ?? "",
        securityToken: headers["x-amz-security-token"] ?? null, headers });
      const host = new URL(request.url).hostname;
      if (host.startsWith("bedrock-runtime.")) {
        if (fake.bedrock === "denied") {
          return new Response(JSON.stringify({ message: "User is not authorized to perform: bedrock:InvokeModelWithResponseStream" }), {
            status: 403, headers: { "content-type": "application/json", "x-amzn-errortype": "AccessDeniedException:http://internal.amazon.com/" },
          });
        }
        return new Response(converseBody(), { status: 200, headers: { "content-type": "application/vnd.amazon.eventstream" } });
      }
      if (host.startsWith("sts.")) {
        const next = fake.sts.shift();
        if (!next || next === "fail") {
          return new Response("<ErrorResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><Error><Type>Sender</Type>" +
            "<Code>AccessDenied</Code><Message>fake STS refused AssumeRole</Message></Error><RequestId>r</RequestId></ErrorResponse>",
            { status: 403, headers: { "content-type": "text/xml" } });
        }
        return new Response("<AssumeRoleResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><AssumeRoleResult><Credentials>" +
          `<AccessKeyId>${next.accessKeyId}</AccessKeyId><SecretAccessKey>role-secret</SecretAccessKey>` +
          `<SessionToken>session-${next.accessKeyId}</SessionToken><Expiration>${new Date(next.expiration).toISOString()}</Expiration>` +
          "</Credentials><AssumedRoleUser><AssumedRoleId>AROAFIXTURE:corral</AssumedRoleId>" +
          "<Arn>arn:aws:sts::111122223333:assumed-role/Dev/corral</Arn></AssumedRoleUser></AssumeRoleResult>" +
          "<ResponseMetadata><RequestId>r</RequestId></ResponseMetadata></AssumeRoleResponse>",
          { status: 200, headers: { "content-type": "text/xml" } });
      }
      if (host.startsWith("portal.sso.")) {
        const next = fake.portal.shift();
        if (!next) return new Response(JSON.stringify({ message: "no queued role" }), { status: 401 });
        return new Response(JSON.stringify({ roleCredentials: { accessKeyId: next.accessKeyId, secretAccessKey: "sso-secret",
          sessionToken: `session-${next.accessKeyId}`, expiration: next.expiration } }),
          { status: 200, headers: { "content-type": "application/json" } });
      }
      throw new Error(`fake AWS has no route for ${request.url}`);
    },
  };
  return fake;
}

export function signingKey(request: SentRequest): string {
  return /Credential=([A-Z0-9]+)\//.exec(request.authorization)?.[1] ?? "";
}

export interface BedrockHost {
  readonly network: GuardedFetch;
  turn(): Promise<AssistantMessage>;
  close(): void;
}

/**
 * A Bedrock host's network and provider, wired as src/host/main.ts does: handler adoption, the
 * profile read before the guard, rules from the binding, globalThis.fetch replaced by the guard and
 * the restricted provider. HOME points at `home` so the SDK clients read only the fixture files.
 * Rejects like the host when the profile is unusable.
 */
export async function bedrockHost(
  home: string, send: FakeAws["send"], options: { readonly now?: () => number; readonly binding?: AwsBinding } = {},
): Promise<BedrockHost> {
  const binding = options.binding ?? BINDING;
  const previous = { fetch: globalThis.fetch, home: process.env.HOME };
  process.env.HOME = home;
  const close = () => {
    globalThis.fetch = previous.fetch;
    process.env.HOME = previous.home;
  };
  try {
    adoptSmithyHandlers();
    const resolver = createAwsCredentialResolver(binding, home, options.now ? { now: options.now } : {});
    const rules = hostRules("amazon-bedrock", bedrockRoute(binding, MODEL, await resolver.profile()));
    const network = guardedFetch(send, undefined, "amazon-bedrock", rules);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    await resolver.get();
    const models = await createModelCatalog(auth.credentials(authContext("amazon-bedrock")),
      { providers: ["amazon-bedrock"], modelsPath: null });
    restrictProvider(models, "amazon-bedrock", network.fetch as typeof globalThis.fetch, undefined, { binding, resolver });
    const model = models.getModel("amazon-bedrock", MODEL);
    if (!model) throw new Error(`fixture model ${MODEL} is not in Pi's catalog`);
    return {
      network, close,
      turn: () => models.complete(model, { messages: [{ role: "user", content: "Reply OK", timestamp: Date.now() }] } as never,
        { maxRetries: 0 } as never),
    };
  } catch (error) {
    close();
    throw error;
  }
}
