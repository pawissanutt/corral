// Credential lifetime with an injected clock (T-504): every cached layer tries to renew from five
// minutes before its Expiration, keeps signing with its unexpired credentials when renewal is
// impossible or fails, and refuses only at the exact expiry, naming `aws sso login` for an SSO root.
import { afterEach, expect, test } from "bun:test";

import {
  CONVERSE_URL, LEGACY_SSO_CONFIG, SSO_SESSION_CONFIG, type SentRequest, awsHome, bedrockHost, fakeAws, signingKey, snapshotAws,
  ssoCacheFile, ssoToken,
} from "./fixtures/bedrock/fake-aws.ts";

const closers: (() => void)[] = [];
afterEach(() => {
  for (const close of closers.splice(0)) close();
});

const MINUTE = 60_000;
const STS = "https://sts.us-west-2.amazonaws.com/";
const PORTAL = "https://portal.sso.us-east-1.amazonaws.com/federation/credentials?account_id=111122223333&role_name=Dev";

function since(requests: readonly SentRequest[], from: number): string[][] {
  return requests.slice(from).map((request) => [request.method, request.url, signingKey(request)]);
}

for (const [name, config, cacheKey] of [
  ["sso_session", SSO_SESSION_CONFIG, "corp"],
  ["legacy sso_start_url", LEGACY_SSO_CONFIG, "https://legacy.awsapps.com/start"],
] as const) {
  test(`${name}: cached role keys sign inside the renewal window after the token expired, and refuse at expiry`, async () => {
    const t0 = Date.now();
    let offset = 0;
    const home = awsHome({ config, [ssoCacheFile(cacheKey)]: ssoToken(t0 + 2 * MINUTE) });
    const before = snapshotAws(home);
    const fake = fakeAws();
    fake.portal.push({ accessKeyId: "ASIASSOKEY0000000001", expiration: t0 + 60 * MINUTE });
    const host = await bedrockHost(home, fake.send, { now: () => t0 + offset });
    closers.push(host.close);
    expect(since(fake.requests, 0)).toEqual([["GET", PORTAL, ""]]);

    offset = 56 * MINUTE;
    expect((await host.turn()).content).toEqual([{ type: "text", text: "OK" }]);
    expect(since(fake.requests, 1)).toEqual([["POST", CONVERSE_URL, "ASIASSOKEY0000000001"]]);

    offset = 60 * MINUTE;
    const refused = await host.turn();
    expect(refused.stopReason).toBe("error");
    expect(refused.errorMessage).toContain("run `aws sso login --profile dev`");
    expect(fake.requests).toHaveLength(2);
    expect(fake.requests.some((request) => new URL(request.url).hostname.startsWith("oidc."))).toBeFalse();
    expect(snapshotAws(home)).toEqual(before);
  });
}

test("an assume-role chain rooted in SSO renews through STS with still-valid source keys, then refuses at expiry", async () => {
  const t0 = Date.now();
  let offset = 0;
  const home = awsHome({
    config: "[profile dev]\nrole_arn = arn:aws:iam::111122223333:role/Builder\nsource_profile = sso-base\n\n" +
      SSO_SESSION_CONFIG.replace("[profile dev]", "[profile sso-base]"),
    [ssoCacheFile("corp")]: ssoToken(t0 + 2 * MINUTE),
  });
  const before = snapshotAws(home);
  const fake = fakeAws();
  fake.portal.push({ accessKeyId: "ASIASSOKEY0000000001", expiration: t0 + 60 * MINUTE });
  fake.sts.push({ accessKeyId: "ASIAROLEKEY000000001", expiration: t0 + 60 * MINUTE },
    { accessKeyId: "ASIAROLEKEY000000002", expiration: t0 + 116 * MINUTE });
  const host = await bedrockHost(home, fake.send, { now: () => t0 + offset });
  closers.push(host.close);
  expect(since(fake.requests, 0)).toEqual([["GET", PORTAL, ""], ["POST", STS, "ASIASSOKEY0000000001"]]);

  offset = 56 * MINUTE;
  await host.turn();
  expect(since(fake.requests, 2)).toEqual([["POST", STS, "ASIASSOKEY0000000001"], ["POST", CONVERSE_URL, "ASIAROLEKEY000000002"]]);

  offset = 111 * MINUTE;
  await host.turn();
  expect(since(fake.requests, 4)).toEqual([["POST", CONVERSE_URL, "ASIAROLEKEY000000002"]]);

  offset = 116 * MINUTE;
  const refused = await host.turn();
  expect(refused.stopReason).toBe("error");
  expect(refused.errorMessage).toContain("run `aws sso login --profile sso-base`");
  expect(fake.requests).toHaveLength(5);
  expect(fake.requests.some((request) => new URL(request.url).hostname.startsWith("oidc."))).toBeFalse();
  expect(snapshotAws(home)).toEqual(before);
});

test("an assume-role chain rooted in static keys keeps its role keys when STS fails in the window, then ends with the STS error", async () => {
  const t0 = Date.now();
  let offset = 0;
  const home = awsHome({
    config: "[profile dev]\nrole_arn = arn:aws:iam::111122223333:role/Builder\nsource_profile = base\n",
    credentials: "[base]\naws_access_key_id = AKIABASEKEY000000001\naws_secret_access_key = base-secret\n",
  });
  const before = snapshotAws(home);
  const fake = fakeAws();
  fake.sts.push({ accessKeyId: "ASIAROLEKEY000000001", expiration: t0 + 60 * MINUTE }, "fail", "fail");
  const host = await bedrockHost(home, fake.send, { now: () => t0 + offset });
  closers.push(host.close);
  expect(since(fake.requests, 0)).toEqual([["POST", STS, "AKIABASEKEY000000001"]]);

  offset = 56 * MINUTE;
  await host.turn();
  expect(since(fake.requests, 1)).toEqual([["POST", STS, "AKIABASEKEY000000001"], ["POST", CONVERSE_URL, "ASIAROLEKEY000000001"]]);

  offset = 60 * MINUTE;
  const refused = await host.turn();
  expect(refused.stopReason).toBe("error");
  expect(refused.errorMessage).toContain("AssumeRole for AWS profile dev failed");
  expect(refused.errorMessage).toContain("fake STS refused AssumeRole");
  expect(since(fake.requests, 3)).toEqual([["POST", STS, "AKIABASEKEY000000001"]]);
  expect(snapshotAws(home)).toEqual(before);
});
