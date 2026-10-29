// Read-only IAM Identity Center (T-504): a cached token two minutes from expiry, with the refresh
// fields the AWS SDK would use to refresh and rewrite it, yields role keys from the fake portal and
// no OIDC request; an expired token is refused naming `aws sso login`. HOME/.aws never changes.
import { afterEach, expect, test } from "bun:test";

import {
  CONVERSE_URL, SSO_SESSION_CONFIG, awsHome, bedrockHost, fakeAws, signingKey, snapshotAws, ssoCacheFile, ssoToken,
} from "./fixtures/bedrock/fake-aws.ts";

const closers: (() => void)[] = [];
afterEach(() => {
  for (const close of closers.splice(0)) close();
});

const PORTAL = "https://portal.sso.us-east-1.amazonaws.com/federation/credentials?account_id=111122223333&role_name=Dev";

test("a near-expiry token signs with the portal's role keys, never calls OIDC and leaves HOME/.aws untouched", async () => {
  const home = awsHome({ config: SSO_SESSION_CONFIG, [ssoCacheFile("corp")]: ssoToken(Date.now() + 120_000) });
  const before = snapshotAws(home);
  const fake = fakeAws();
  fake.portal.push({ accessKeyId: "ASIASSOKEY0000000001", expiration: Date.now() + 3_600_000 });
  const host = await bedrockHost(home, fake.send);
  closers.push(host.close);
  expect((await host.turn()).content).toEqual([{ type: "text", text: "OK" }]);
  expect(fake.requests.map((request) => [request.method, request.url])).toEqual([["GET", PORTAL], ["POST", CONVERSE_URL]]);
  expect(fake.requests[0]!.headers["x-amz-sso_bearer_token"]).toBe("fixture-sso-access-token");
  expect(signingKey(fake.requests[1]!)).toBe("ASIASSOKEY0000000001");
  expect(fake.requests[1]!.securityToken).toBe("session-ASIASSOKEY0000000001");
  expect(fake.requests.some((request) => new URL(request.url).hostname.startsWith("oidc."))).toBeFalse();
  expect(snapshotAws(home)).toEqual(before);
});

test("an expired or missing token stops the host naming aws sso login before any request", async () => {
  for (const files of [
    { config: SSO_SESSION_CONFIG, [ssoCacheFile("corp")]: ssoToken(Date.now() - 60_000) },
    { config: SSO_SESSION_CONFIG },
  ]) {
    const home = awsHome(files);
    const before = snapshotAws(home);
    const fake = fakeAws();
    let error: (Error & { code?: string }) | undefined;
    try {
      closers.push((await bedrockHost(home, fake.send)).close);
    } catch (caught) {
      error = caught as Error;
    }
    expect(error?.code).toBe("credentials_unavailable");
    expect(error?.message).toContain("run `aws sso login --profile dev`");
    expect(fake.requests).toEqual([]);
    expect(snapshotAws(home)).toEqual(before);
  }
});

test("role keys that expire mid-session after the token expired end the turn with the same refusal", async () => {
  const start = Date.now();
  let offset = 0;
  const home = awsHome({ config: SSO_SESSION_CONFIG, [ssoCacheFile("corp")]: ssoToken(start + 120_000) });
  const before = snapshotAws(home);
  const fake = fakeAws();
  fake.portal.push({ accessKeyId: "ASIASSOKEY0000000001", expiration: start + 3_600_000 });
  const host = await bedrockHost(home, fake.send, { now: () => start + offset });
  closers.push(host.close);
  offset = 3_600_000;
  const reply = await host.turn();
  expect(reply.stopReason).toBe("error");
  expect(reply.errorMessage).toContain("credentials_unavailable");
  expect(reply.errorMessage).toContain("run `aws sso login --profile dev`");
  expect(fake.requests.map((request) => request.url)).toEqual([PORTAL]);
  expect(snapshotAws(home)).toEqual(before);
});
