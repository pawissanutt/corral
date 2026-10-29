// Bedrock credentials come only from the bound AWS profile (OD-2, T-504): ambient AWS_* canaries never
// sign or appear, a missing profile or an unsupported mechanism stops before any request or
// subprocess, assume-role goes through the fake STS, and a Bedrock host's subprocesses see no AWS_*.
import { afterEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { LaunchMessageInvalid, parseLaunchMessage } from "../src/host/config.ts";
import { restrictEnvironment } from "../src/host/restrict.ts";
import {
  CONVERSE_URL, FIXTURE_KEY, STATIC_DEV, awsHome, bedrockHost, fakeAws, signingKey,
} from "./fixtures/bedrock/fake-aws.ts";

const closers: (() => void)[] = [];
const savedAws = Object.fromEntries(Object.entries(process.env).filter(([key]) => key.startsWith("AWS_")));
afterEach(() => {
  for (const close of closers.splice(0)) close();
  for (const key of Object.keys(process.env)) if (key.startsWith("AWS_")) delete process.env[key];
  Object.assign(process.env, savedAws);
});

async function rejection(promise: Promise<unknown>): Promise<Error> {
  try {
    await promise;
  } catch (error) {
    return error as Error;
  }
  throw new Error("expected a rejection");
}

test("ambient AWS_* canaries never sign, appear in a request or redirect the client", async () => {
  const canaryDir = mkdtempSync(join(tmpdir(), "corral-canary-"));
  const canaryConfig = join(canaryDir, "config");
  writeFileSync(canaryConfig, "[default]\nuse_fips_endpoint = true\n\n[profile dev]\naws_access_key_id = AKIACANARYCONFIG0000\n" +
    "aws_secret_access_key = canary-config-secret\n");
  Object.assign(process.env, {
    AWS_ACCESS_KEY_ID: "AKIACANARY000000000", AWS_SECRET_ACCESS_KEY: "canary-secret", AWS_SESSION_TOKEN: "canary-session",
    AWS_PROFILE: "other", AWS_CONFIG_FILE: canaryConfig, AWS_SHARED_CREDENTIALS_FILE: canaryConfig,
    AWS_BEARER_TOKEN_BEDROCK: "canary", AWS_REGION: "eu-central-1",
  });
  restrictEnvironment(process.env, "amazon-bedrock");
  expect(Object.keys(process.env).filter((key) => key.startsWith("AWS_"))).toEqual([]);
  const fake = fakeAws();
  const host = await bedrockHost(awsHome(STATIC_DEV), fake.send);
  closers.push(host.close);
  expect((await host.turn()).content).toEqual([{ type: "text", text: "OK" }]);
  expect(fake.requests.map((request) => request.url)).toEqual([CONVERSE_URL]);
  expect(signingKey(fake.requests[0]!)).toBe(FIXTURE_KEY);
  const sent = JSON.stringify(fake.requests);
  for (const canary of ["AKIACANARY", "canary-secret", "canary-session", "canary-config-secret", "Bearer"]) {
    expect([canary, sent.includes(canary)]).toEqual([canary, false]);
  }
});

test("a missing profile stops credentials_unavailable naming the profile and the login, before any request", async () => {
  const fake = fakeAws();
  const error = await rejection(bedrockHost(awsHome({ credentials: "[other]\naws_access_key_id = AKIAOTHER\naws_secret_access_key = s\n" }),
    fake.send));
  expect((error as Error & { code?: string }).code).toBe("credentials_unavailable");
  expect(error.message).toContain("AWS profile dev is not in ~/.aws/config or ~/.aws/credentials");
  expect(error.message).toContain("corral agent login --provider amazon-bedrock");
  expect(fake.requests).toEqual([]);
});

test("assume-role signs Bedrock with the fake STS's role keys after one allowed STS POST", async () => {
  const fake = fakeAws();
  fake.sts.push({ accessKeyId: "ASIAROLEKEY000000001", expiration: Date.now() + 3_600_000 });
  const host = await bedrockHost(awsHome({
    config: "[profile dev]\nrole_arn = arn:aws:iam::111122223333:role/Dev\nsource_profile = base\nexternal_id = fixture-external\n",
    credentials: "[base]\naws_access_key_id = AKIABASEKEY000000001\naws_secret_access_key = base-secret\n",
  }), fake.send);
  closers.push(host.close);
  expect((await host.turn()).content).toEqual([{ type: "text", text: "OK" }]);
  expect(fake.requests.map((request) => [request.method, request.url, signingKey(request)])).toEqual([
    ["POST", "https://sts.us-west-2.amazonaws.com/", "AKIABASEKEY000000001"],
    ["POST", CONVERSE_URL, "ASIAROLEKEY000000001"],
  ]);
  expect(fake.requests[1]!.securityToken).toBe("session-ASIAROLEKEY000000001");
  expect(host.network.ledger()).toEqual({ allowed: 2, refused: 0, refusedDestinations: [] });
});

test("a profile without role_arn cannot reach STS", async () => {
  const fake = fakeAws();
  const host = await bedrockHost(awsHome(STATIC_DEV), fake.send);
  closers.push(host.close);
  await expect(host.network.fetch("https://sts.us-west-2.amazonaws.com/", { method: "POST", body: "Action=AssumeRole" }))
    .rejects.toThrow(/^network_refused: https:\/\/sts\.us-west-2\.amazonaws\.com\//);
  await expect(host.network.fetch("https://sts.amazonaws.com/", { method: "POST" })).rejects.toThrow(/^network_refused: /);
  expect(fake.requests).toEqual([]);
});

test("each unsupported mechanism is refused by name, with no request and no subprocess", async () => {
  const marker = join(mkdtempSync(join(tmpdir(), "corral-marker-")), "ran");
  const base = "[profile base]\naws_access_key_id = AKIABASEKEY000000001\naws_secret_access_key = base-secret\n\n";
  const cases: Record<string, string> = {
    credential_process: `[profile dev]\ncredential_process = /bin/sh -c 'touch ${marker}'\n`,
    web_identity_token_file: "[profile dev]\nrole_arn = arn:aws:iam::111122223333:role/Dev\nweb_identity_token_file = /tmp/token\n",
    credential_source: "[profile dev]\nrole_arn = arn:aws:iam::111122223333:role/Dev\ncredential_source = Ec2InstanceMetadata\n",
    mfa_serial: `${base}[profile dev]\nrole_arn = arn:aws:iam::111122223333:role/Dev\nsource_profile = base\n` +
      "mfa_serial = arn:aws:iam::111122223333:mfa/dev\n",
    login_session: "[profile dev]\nlogin_session = arn:aws:iam::111122223333:user/dev\n",
  };
  for (const [key, config] of Object.entries(cases)) {
    const fake = fakeAws();
    const error = await rejection(bedrockHost(awsHome({ config }), fake.send));
    expect([key, (error as Error & { code?: string }).code, error.message.includes(`uses ${key}, which corral does not support`)])
      .toEqual([key, "credentials_unavailable", true]);
    expect(fake.requests).toEqual([]);
  }
  expect(existsSync(marker)).toBeFalse();
});

test("a role chain longer than four steps or with a cycle is refused", async () => {
  const chain = Array.from({ length: 5 }, (_, index) =>
    `[profile ${index === 0 ? "dev" : `r${index}`}]\nrole_arn = arn:aws:iam::111122223333:role/R${index}\nsource_profile = r${index + 1}\n`)
    .join("\n") + "\n[profile r5]\naws_access_key_id = AKIAR5\naws_secret_access_key = s\n";
  expect((await rejection(bedrockHost(awsHome({ config: chain }), fakeAws().send))).message).toContain("chains more than 4 roles");
  const cycle = "[profile dev]\nrole_arn = arn:aws:iam::1:role/A\nsource_profile = b\n\n[profile b]\nrole_arn = arn:aws:iam::1:role/B\n" +
    "source_profile = dev\n";
  expect((await rejection(bedrockHost(awsHome({ config: cycle }), fakeAws().send))).message).toContain("source_profile cycle");
});

test("the launch message carries an optional non-secret aws binding; older messages stay valid", () => {
  const message = {
    version: 1, store: "st-demo", project: "demo", role: "worker", task: "T-001", run: "R1", generation: 1,
    secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
    pane: "p2", cwd: "/work", root: "/work", corral: "/bin/corral", assets: "/assets", runtime_dir: "/runtime",
    session_dir: "/sessions", model: "us.anthropic.claude-sonnet-4-5-20250929-v1:0", provider: "amazon-bedrock",
  };
  const parse = (extra: Record<string, unknown>) => parseLaunchMessage(JSON.stringify({ ...message, ...extra }));
  expect(parse({}).aws).toBeUndefined();
  expect(parse({ aws: null }).aws).toBeUndefined();
  expect(parse({ aws: { profile: "dev", region: "us-west-2" } }).aws).toEqual({ profile: "dev", region: "us-west-2" });
  expect(parse({ aws: { profile: "team.ci+bot@corp", region: "ap-southeast-2" } }).aws?.profile).toBe("team.ci+bot@corp");
  for (const aws of ["dev", { profile: "dev" }, { profile: "dev", region: "us-gov-west-1" }, { profile: "dev", region: "cn-north-1" },
    { profile: "dev", region: "US-WEST-2" }, { profile: "dev profile", region: "us-west-2" }, { profile: "x".repeat(65), region: "us-west-2" }]) {
    expect(() => parse({ aws })).toThrow(LaunchMessageInvalid);
  }
});

test("a Bedrock host's subprocesses see no AWS_* variable; other providers keep theirs", () => {
  const restrict = fileURLToPath(new URL("../src/host/restrict.ts", import.meta.url));
  const script = `const { restrictEnvironment } = await import(${JSON.stringify(restrict)});
restrictEnvironment(process.env, process.argv[1]);
const { spawnSync } = await import("node:child_process");
process.stdout.write(spawnSync("env", [], { encoding: "utf8" }).stdout);`;
  const run = (provider: string) => spawnSync(process.execPath, ["--no-env-file", "-e", script, provider], {
    encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 30_000,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, AWS_ACCESS_KEY_ID: "AKIACANARY000000000", AWS_PROFILE: "other",
      AWS_REGION: "eu-central-1", AWS_BEARER_TOKEN_BEDROCK: "canary" },
  });
  const bedrock = run("amazon-bedrock");
  expect([bedrock.status, bedrock.stderr]).toEqual([0, ""]);
  expect(bedrock.stdout).toContain("PI_OFFLINE=1");
  expect(bedrock.stdout.split("\n").filter((line) => line.startsWith("AWS_"))).toEqual([]);
  const codex = run("openai-codex");
  expect(codex.stdout.split("\n").filter((line) => line.startsWith("AWS_")).sort()).toEqual([
    "AWS_ACCESS_KEY_ID=AKIACANARY000000000", "AWS_BEARER_TOKEN_BEDROCK=canary", "AWS_PROFILE=other", "AWS_REGION=eu-central-1",
  ]);
});
