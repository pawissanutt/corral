// The Bedrock network guard (T-504): a fake-transport ConverseStream turn through pi-ai's real
// Bedrock client and the adopted smithy handlers, every refused destination, the socket tripwire,
// and the host's classification of an AWS 403.
import { afterEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeHttp2Handler, NodeHttpHandler } from "@smithy/node-http-handler";

import { hostRules } from "../src/auth/providers.ts";
import { NetworkRefused, guardedFetch } from "../src/host/restrict.ts";
import { providerErrorClass } from "../src/host/session.ts";
import { adoptSmithyHandlers } from "../src/host/smithy.ts";
import {
  CONVERSE_URL, FIXTURE_KEY, MODEL, REGION, STATIC_DEV, awsHome, bedrockHost, fakeAws, signingKey,
} from "./fixtures/bedrock/fake-aws.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

const closers: (() => void)[] = [];
afterEach(() => {
  for (const close of closers.splice(0)) close();
});

const STATIC_ROUTE = { region: REGION, model: MODEL, assumesRole: false, ssoRegions: [] };
const SIGV4 = { authorization: `AWS4-HMAC-SHA256 Credential=${FIXTURE_KEY}/20261029/${REGION}/bedrock/aws4_request, ` +
  "SignedHeaders=host, Signature=00" };

test("one SigV4-signed POST reaches the bound ConverseStream URL and the event stream decodes", async () => {
  const fake = fakeAws();
  const host = await bedrockHost(awsHome(STATIC_DEV), fake.send);
  closers.push(host.close);
  const reply = await host.turn();
  expect(reply.stopReason).toBe("stop");
  expect(reply.content).toEqual([{ type: "text", text: "OK" }]);
  expect([reply.usage.input, reply.usage.output, reply.usage.totalTokens]).toEqual([11, 2, 13]);
  expect(fake.requests.map((request) => [request.method, request.url])).toEqual([["POST", CONVERSE_URL]]);
  expect(fake.requests[0]!.authorization).toMatch(new RegExp(`^AWS4-HMAC-SHA256 Credential=${FIXTURE_KEY}/`));
  expect(signingKey(fake.requests[0]!)).toBe(FIXTURE_KEY);
  expect(host.network.ledger()).toEqual({ allowed: 1, refused: 0, refusedDestinations: [] });
  expect(host.network.destinations).toEqual([CONVERSE_URL]);
});

test("every other host, region, model path, method, OIDC token request and Bearer header is refused unsent", async () => {
  const sent: string[] = [];
  const refusedEvents: string[] = [];
  const guard = guardedFetch(async (input) => {
    sent.push(input instanceof Request ? input.url : String(input));
    return new Response("{}");
  }, (destination) => refusedEvents.push(destination), "amazon-bedrock", hostRules("amazon-bedrock", STATIC_ROUTE));
  const post = { method: "POST", headers: SIGV4, body: "{}" };
  const refused: [string, RequestInit][] = [
    [`https://bedrock-runtime.us-east-1.amazonaws.com/model/${encodeURIComponent(MODEL)}/converse-stream`, post],
    [`https://bedrock-runtime-fips.us-west-2.amazonaws.com/model/${encodeURIComponent(MODEL)}/converse-stream`, post],
    [`https://bedrock-agent-runtime.us-west-2.amazonaws.com/model/${encodeURIComponent(MODEL)}/converse-stream`, post],
    ["https://bedrock-runtime.us-west-2.amazonaws.com/model/anthropic.claude-3-haiku-20240307-v1%3A0/converse-stream", post],
    [`https://bedrock-runtime.us-west-2.amazonaws.com/model/${encodeURIComponent(MODEL)}/converse`, post],
    [`${CONVERSE_URL}?x=1`, post],
    ["https://oidc.us-east-1.amazonaws.com/token", post],
    ["http://169.254.169.254/latest/api/token", { method: "PUT" }],
    ["https://api.anthropic.com/v1/messages", post],
    ["https://example.com/", post],
    ["https://sts.us-west-2.amazonaws.com/", post],
    ["https://portal.sso.us-east-1.amazonaws.com/federation/credentials", { method: "GET" }],
    [CONVERSE_URL, { method: "GET", headers: SIGV4 }],
    [CONVERSE_URL, { method: "POST", headers: { authorization: "Bearer bedrock-api-key" }, body: "{}" }],
    [CONVERSE_URL, { method: "POST", body: "{}" }],
  ];
  for (const [url, init] of refused) {
    let error: unknown;
    try {
      await guard.fetch(url, init);
    } catch (caught) {
      error = caught;
    }
    expect([url, error instanceof NetworkRefused]).toEqual([url, true]);
  }
  expect(sent).toEqual([]);
  expect(guard.ledger().refused).toBe(refused.length);
  expect(guard.ledger().refusedDestinations).toContain("https://oidc.us-east-1.amazonaws.com/token");
  expect(guard.ledger().refusedDestinations).not.toContain(`${CONVERSE_URL}?x=1`);
  expect(refusedEvents).toHaveLength(refused.length);
  await guard.fetch(new Request(CONVERSE_URL, { method: "POST", headers: SIGV4, body: "{}" }));
  expect(sent).toEqual([CONVERSE_URL]);
  expect(guard.ledger().allowed).toBe(1);
});

test("a host without a Bedrock binding has no allowed destination, and STS and the portal follow the profile", () => {
  expect(hostRules("amazon-bedrock")).toEqual([]);
  expect(hostRules("amazon-bedrock", { ...STATIC_ROUTE, region: "us-gov-west-1" })).toEqual([]);
  expect(hostRules("amazon-bedrock", { ...STATIC_ROUTE, region: "cn-north-1" })).toEqual([]);
  expect(hostRules("amazon-bedrock", { ...STATIC_ROUTE, assumesRole: true, ssoRegions: ["eu-west-1"] })
    .map((rule) => `${rule.method} ${rule.display}`)).toEqual([
    `POST ${CONVERSE_URL}`, "POST https://sts.amazonaws.com/", "POST https://sts.us-west-2.amazonaws.com/",
    "GET https://portal.sso.eu-west-1.amazonaws.com/federation/credentials",
  ]);
});

test("raw node:https, node:http, node:http2, node:tls and node:net TCP connects are refused; Unix sockets work", async () => {
  const child = spawnSync(process.execPath,
    ["--no-env-file", fileURLToPath(new URL("fixtures/bedrock/tripwire.ts", import.meta.url))],
    { stdio: ["ignore", "pipe", "pipe"], encoding: "utf8", timeout: 60_000 });
  expect([child.status, child.stderr]).toEqual([0, ""]);
  const { results, ledger } = JSON.parse(child.stdout) as { results: Record<string, string>; ledger: { allowed: number; refused: number;
    refusedDestinations: string[] } };
  expect(results).toEqual({
    "https.request": "network_refused", "https.get": "network_refused", "http.request": "network_refused",
    "http2.connect": "network_refused", "tls.connect": "network_refused", "net.connect": "network_refused",
    "net.createConnection": "network_refused", "Socket.connect": "network_refused", unix: "pong",
  });
  expect(ledger.allowed).toBe(0);
  expect(ledger.refused).toBe(8);
  expect(ledger.refusedDestinations).toEqual([
    "https://bedrock-runtime.us-west-2.amazonaws.invalid/model/x/converse-stream", "https://127.0.0.1:9/",
    "http://127.0.0.1:9/latest/api/token", "https://bedrock-runtime.us-west-2.amazonaws.invalid/", "tcp://127.0.0.1:9",
  ]);
});

test("exactly one @smithy/node-http-handler is installed, and the Bedrock and AWS clients resolve the adopted copy", () => {
  const modules = join(PACKAGE_ROOT, "node_modules");
  const copies: string[] = [];
  const walk = (dir: string, depth: number) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const path = join(dir, entry.name);
      if (path.endsWith(join("@smithy", "node-http-handler"))) copies.push(path);
      else if (depth < 6) walk(path, depth + 1);
    }
  };
  walk(modules, 0);
  expect(copies).toEqual([join(modules, "@smithy/node-http-handler")]);
  const resolvedFrom = (root: string) => dirname(createRequire(join(root, "package.json")).resolve("@smithy/node-http-handler"));
  const own = resolvedFrom(PACKAGE_ROOT);
  for (const pkg of ["@aws-sdk/client-bedrock-runtime", "@aws-sdk/nested-clients", "@earendil-works/pi-ai"]) {
    expect([pkg, resolvedFrom(join(modules, pkg))]).toEqual([pkg, own]);
  }
  adoptSmithyHandlers();
  const adoptedHandle = (NodeHttpHandler.prototype as unknown as { handle: unknown }).handle;
  expect((NodeHttp2Handler.prototype as unknown as { handle: unknown }).handle).toBe(adoptedHandle);
});

test("models.ts --provider amazon-bedrock lists at least 190 converse-stream selectors offline, with no us-gov. ID", () => {
  const run = spawnSync(process.execPath, ["--no-env-file", join(PACKAGE_ROOT, "src/auth/models.ts"), "--provider", "amazon-bedrock"], {
    encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 60_000,
    env: { PATH: process.env.PATH, HOME: "/nonexistent-corral-home" },
  });
  expect([run.status, run.stderr]).toEqual([0, ""]);
  const models = (JSON.parse(run.stdout) as { models: { selector: string; provider: string; id: string }[] }).models;
  expect(models.length).toBeGreaterThanOrEqual(190);
  expect(models.every((model) => model.provider === "amazon-bedrock" && model.selector === `amazon-bedrock/${model.id}`)).toBeTrue();
  for (const prefix of ["us.", "eu.", "apac.", "global."]) {
    expect([prefix, models.some((model) => model.id.startsWith(prefix))]).toEqual([prefix, true]);
  }
  expect(models.filter((model) => model.id.startsWith("us-gov."))).toEqual([]);
  expect(models.map((model) => model.id)).toContain(MODEL);
});

test("scripts/bedrock-smoke.sh without --yes-metered prints its plan and exits non-zero without network", () => {
  const run = spawnSync(join(PACKAGE_ROOT, "../../scripts/bedrock-smoke.sh"),
    ["--aws-profile", "dev", "--region", REGION, "--model", MODEL], {
      encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 60_000,
      env: { PATH: process.env.PATH, HOME: "/nonexistent-corral-home" },
    });
  expect(run.status).toBe(3);
  expect(run.stdout).toBe([`endpoint: POST ${CONVERSE_URL}`, "aws profile: dev", `region: ${REGION}`, `model: ${MODEL}`,
    "output token cap: 16", "billing: metered per token to the AWS account of that profile",
    "not sent: pass --yes-metered to send this one metered request", ""].join("\n"));
});

test("a 403 AccessDeniedException ends the turn as an auth provider error without any credential", async () => {
  const fake = fakeAws();
  fake.bedrock = "denied";
  const host = await bedrockHost(awsHome(STATIC_DEV), fake.send);
  closers.push(host.close);
  const reply = await host.turn();
  expect(reply.stopReason).toBe("error");
  expect(reply.errorMessage).toContain("AccessDeniedException");
  expect(reply.errorMessage).not.toContain(FIXTURE_KEY);
  expect(reply.errorMessage).not.toContain("fixture-secret");
  expect(providerErrorClass(reply.errorMessage ?? "")).toBe("auth");
  expect(fake.requests).toHaveLength(1);
  for (const [text, kind] of [
    ["AccessDeniedException", "auth"], ["UnrecognizedClientException: The security token included in the request is invalid", "auth"],
    ["ExpiredTokenException", "auth"], ["InvalidSignatureException", "auth"], ["ThrottlingException", "rate_limit"],
    ["Throttling error: Too many requests", "rate_limit"], ["ServiceUnavailableException", "overload"],
    ["ModelNotReadyException", "overload"], ["credentials_unavailable: AWS profile dev is not in ~/.aws/config", "auth"],
  ] as const) {
    expect([text, providerErrorClass(text)]).toEqual([text, kind]);
  }
});
