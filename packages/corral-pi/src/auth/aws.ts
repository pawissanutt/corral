// The Bedrock host's credentials (OD-2, T-504): only the named AWS profile the binding records, read
// from $HOME/.aws/config and $HOME/.aws/credentials, through exactly three mechanisms: static keys,
// assume-role through source_profile (at most four role steps, no cycles), and IAM Identity
// Center with the token the user's own `aws sso login` cached. It is read-only: it never refreshes
// an SSO token, never calls the SSO OIDC endpoint and never writes under ~/.aws, so the AWS SDK's
// refreshing fromIni/fromSso chain is not used. The only network calls are SSO GetRoleCredentials
// and STS AssumeRole, which go through the host's guarded fetch.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { GetRoleCredentialsCommand, SSOClient } from "@aws-sdk/nested-clients/sso";
import { AssumeRoleCommand, STSClient } from "@aws-sdk/nested-clients/sts";
import { loadSsoSessionData, parseKnownFiles } from "@smithy/core/config";

import { CredentialsUnavailable } from "./paths.ts";
import { type BedrockRoute, isAwsRegion } from "./providers.ts";

export interface AwsBinding {
  readonly profile: string;
  readonly region: string;
}

export interface AwsCredentials {
  readonly accessKeyId: string;
  readonly secretAccessKey: string;
  readonly sessionToken?: string;
  readonly expiration?: Date;
}

/** From this long before Expiration a layer tries, once per turn, to renew its credentials. */
export const RENEWAL_WINDOW_MS = 300_000;
export const MAX_ROLE_STEPS = 4;
export const UNSUPPORTED_KEYS = ["credential_process", "web_identity_token_file", "credential_source", "mfa_serial",
  "login_session"] as const;

export type AwsStep =
  | { readonly kind: "static"; readonly profile: string; readonly credentials: AwsCredentials }
  | {
    readonly kind: "sso"; readonly profile: string; readonly cacheKey: string; readonly region: string;
    readonly accountId: string; readonly roleName: string;
  }
  | {
    readonly kind: "assume-role"; readonly profile: string; readonly roleArn: string; readonly externalId?: string;
    readonly sessionName?: string; readonly durationSeconds?: number; readonly source: AwsStep;
  };

export interface AwsProfileChain {
  readonly root: AwsStep;
  readonly assumesRole: boolean;
  readonly ssoRegions: readonly string[];
}

type Section = Record<string, string | undefined>;

function unavailable(reason: string): CredentialsUnavailable {
  return new CredentialsUnavailable(reason);
}

function ssoStep(name: string, section: Section, sessions: Record<string, Section>): AwsStep {
  let startUrl: string | undefined;
  let region: string | undefined;
  let cacheKey: string;
  if (section.sso_session !== undefined) {
    const session = sessions[section.sso_session];
    if (!session) {
      throw unavailable(`AWS profile ${name} names sso_session ${section.sso_session}, which has no [sso-session] section`);
    }
    startUrl = session.sso_start_url;
    region = session.sso_region;
    cacheKey = section.sso_session;
  } else {
    startUrl = section.sso_start_url;
    region = section.sso_region;
    cacheKey = startUrl ?? "";
  }
  const missing = [["sso_start_url", startUrl], ["sso_region", region], ["sso_account_id", section.sso_account_id],
    ["sso_role_name", section.sso_role_name]].find(([, value]) => !value)?.[0];
  if (missing !== undefined) throw unavailable(`AWS profile ${name} uses IAM Identity Center but has no ${missing}`);
  if (!isAwsRegion(region ?? "")) throw unavailable(`AWS profile ${name} has an sso_region outside the aws partition`);
  return { kind: "sso", profile: name, cacheKey, region: region!, accountId: section.sso_account_id!,
    roleName: section.sso_role_name! };
}

function step(
  name: string, profiles: Record<string, Section>, sessions: Record<string, Section>, roles: number, seen: readonly string[],
): AwsStep {
  const section = profiles[name];
  if (!section) {
    throw unavailable(`AWS profile ${name} is not in ~/.aws/config or ~/.aws/credentials; run ` +
      `\`aws configure --profile ${name}\`, or bind another profile with \`corral agent login --provider amazon-bedrock\``);
  }
  const unsupported = UNSUPPORTED_KEYS.find((key) => section[key] !== undefined);
  if (unsupported !== undefined) {
    throw unavailable(`AWS profile ${name} uses ${unsupported}, which corral does not support; use static keys, ` +
      "assume-role through source_profile, or IAM Identity Center (SSO)");
  }
  if (section.role_arn !== undefined) {
    const source = section.source_profile;
    if (!source) throw unavailable(`AWS profile ${name} has role_arn without source_profile`);
    if (roles >= MAX_ROLE_STEPS) throw unavailable(`AWS profile ${name} chains more than ${MAX_ROLE_STEPS} roles`);
    if (source === name || seen.includes(source)) throw unavailable(`AWS profile ${name} has a source_profile cycle`);
    const duration = section.duration_seconds === undefined ? undefined : Number(section.duration_seconds);
    if (duration !== undefined && !Number.isSafeInteger(duration)) {
      throw unavailable(`AWS profile ${name} has a non-numeric duration_seconds`);
    }
    return {
      kind: "assume-role", profile: name, roleArn: section.role_arn,
      ...(section.external_id ? { externalId: section.external_id } : {}),
      ...(section.role_session_name ? { sessionName: section.role_session_name } : {}),
      ...(duration !== undefined ? { durationSeconds: duration } : {}),
      source: step(source, profiles, sessions, roles + 1, [...seen, name]),
    };
  }
  if (section.aws_access_key_id && section.aws_secret_access_key) {
    return { kind: "static", profile: name, credentials: {
      accessKeyId: section.aws_access_key_id, secretAccessKey: section.aws_secret_access_key,
      ...(section.aws_session_token ? { sessionToken: section.aws_session_token } : {}),
    } };
  }
  if (section.sso_session !== undefined || section.sso_start_url !== undefined) return ssoStep(name, section, sessions);
  throw unavailable(`AWS profile ${name} has no static keys, role_arn or IAM Identity Center settings`);
}

/** Reads and classifies the bound profile's chain offline; refuses an unusable or unsupported one. */
export async function readAwsProfile(binding: AwsBinding, home: string): Promise<AwsProfileChain> {
  const files = { filepath: join(home, ".aws", "credentials"), configFilepath: join(home, ".aws", "config"),
    ignoreCache: true };
  const profiles = await parseKnownFiles({ ...files, profile: binding.profile }) as Record<string, Section>;
  const sessions = await loadSsoSessionData(files) as Record<string, Section>;
  const root = step(binding.profile, profiles, sessions, 0, []);
  const ssoRegions: string[] = [];
  let assumesRole = false;
  for (let current: AwsStep | undefined = root; current; current = current.kind === "assume-role" ? current.source : undefined) {
    if (current.kind === "assume-role") assumesRole = true;
    if (current.kind === "sso") ssoRegions.push(current.region);
  }
  return { root, assumesRole, ssoRegions };
}

export function bedrockRoute(binding: AwsBinding, model: string, chain: AwsProfileChain): BedrockRoute {
  return { region: binding.region, model, assumesRole: chain.assumesRole, ssoRegions: chain.ssoRegions };
}

interface CachedToken {
  readonly accessToken: string;
  readonly expiresAt: number;
}

/** The token `aws sso login` cached for a session name or legacy start URL; read-only. */
export function readSsoToken(home: string, cacheKey: string): CachedToken | undefined {
  const file = join(home, ".aws", "sso", "cache", `${createHash("sha1").update(cacheKey).digest("hex")}.json`);
  try {
    const value = JSON.parse(readFileSync(file, "utf8")) as { accessToken?: unknown; expiresAt?: unknown };
    const expiresAt = typeof value.expiresAt === "string" ? Date.parse(value.expiresAt) : Number.NaN;
    if (typeof value.accessToken !== "string" || value.accessToken === "" || Number.isNaN(expiresAt)) return undefined;
    return { accessToken: value.accessToken, expiresAt };
  } catch {
    return undefined;
  }
}

function failure(action: string, profile: string, error: unknown): CredentialsUnavailable {
  if (error instanceof CredentialsUnavailable) return error;
  const name = error instanceof Error ? error.name : "Error";
  const message = error instanceof Error ? error.message.slice(0, 200) : String(error).slice(0, 200);
  return unavailable(`${action} for AWS profile ${profile} failed: ${name}: ${message}`);
}

interface Layer {
  get(): Promise<AwsCredentials>;
}

/**
 * One cached credential layer. Credentials are expired when now >= Expiration; from the renewal
 * window on, `renew` is tried once per get(), and its skip or failure keeps the unexpired cache.
 * Only a layer without unexpired credentials resolves afresh and propagates a refusal.
 */
function cachedLayer(now: () => number, resolve: () => Promise<AwsCredentials>,
  renew: () => Promise<AwsCredentials | undefined>): Layer {
  let cached: AwsCredentials | undefined;
  return {
    async get() {
      const at = now();
      const expires = cached?.expiration?.getTime();
      if (cached && expires === undefined) return cached;
      if (cached && expires !== undefined && at < expires - RENEWAL_WINDOW_MS) return cached;
      if (cached && expires !== undefined && at < expires) {
        try {
          cached = (await renew()) ?? cached;
        } catch {
          // Best effort: the unexpired credentials keep signing until their own expiry.
        }
        return cached;
      }
      cached = await resolve();
      return cached;
    },
  };
}

function layer(current: AwsStep, binding: AwsBinding, home: string, now: () => number): Layer {
  if (current.kind === "static") return { get: async () => current.credentials };
  if (current.kind === "sso") {
    const client = new SSOClient({ region: current.region });
    const portal = async (accessToken: string): Promise<AwsCredentials> => {
      try {
        const { roleCredentials: role } = await client.send(new GetRoleCredentialsCommand({
          accessToken, accountId: current.accountId, roleName: current.roleName,
        }));
        if (!role?.accessKeyId || !role.secretAccessKey || role.expiration === undefined) {
          throw unavailable(`the SSO portal returned no role credentials for AWS profile ${current.profile}`);
        }
        return { accessKeyId: role.accessKeyId, secretAccessKey: role.secretAccessKey,
          ...(role.sessionToken ? { sessionToken: role.sessionToken } : {}), expiration: new Date(role.expiration) };
      } catch (error) {
        throw failure("GetRoleCredentials", current.profile, error);
      }
    };
    const token = () => {
      const cached = readSsoToken(home, current.cacheKey);
      return cached && cached.expiresAt > now() ? cached : undefined;
    };
    return cachedLayer(now, async () => {
      const valid = token();
      if (!valid) {
        throw unavailable(`the SSO session for AWS profile ${current.profile} expired or is missing; ` +
          `run \`aws sso login --profile ${current.profile}\``);
      }
      return portal(valid.accessToken);
    }, async () => {
      const valid = token();
      return valid ? portal(valid.accessToken) : undefined;
    });
  }
  const source = layer(current.source, binding, home, now);
  const assume = async (): Promise<AwsCredentials> => {
    const credentials = await source.get();
    try {
      const client = new STSClient({ region: binding.region, credentials });
      const { Credentials: issued } = await client.send(new AssumeRoleCommand({
        RoleArn: current.roleArn, RoleSessionName: current.sessionName ?? `corral-${now()}`,
        ...(current.externalId ? { ExternalId: current.externalId } : {}),
        ...(current.durationSeconds !== undefined ? { DurationSeconds: current.durationSeconds } : {}),
      }));
      if (!issued?.AccessKeyId || !issued.SecretAccessKey || !issued.Expiration) {
        throw unavailable(`STS returned no role credentials for AWS profile ${current.profile}`);
      }
      return { accessKeyId: issued.AccessKeyId, secretAccessKey: issued.SecretAccessKey,
        ...(issued.SessionToken ? { sessionToken: issued.SessionToken } : {}), expiration: issued.Expiration };
    } catch (error) {
      throw failure("AssumeRole", current.profile, error);
    }
  };
  return cachedLayer(now, assume, assume);
}

export interface AwsCredentialResolver {
  profile(): Promise<AwsProfileChain>;
  get(): Promise<AwsCredentials>;
}

/** A per-host resolver; `now` is injectable so tests can drive the renewal window and expiry. */
export function createAwsCredentialResolver(
  binding: AwsBinding, home: string, options: { readonly now?: () => number } = {},
): AwsCredentialResolver {
  const now = options.now ?? Date.now;
  let chain: Promise<AwsProfileChain> | undefined;
  let root: Layer | undefined;
  const profile = () => (chain ??= readAwsProfile(binding, home));
  return {
    profile,
    async get() {
      root ??= layer((await profile()).root, binding, home, now);
      return root.get();
    },
  };
}
