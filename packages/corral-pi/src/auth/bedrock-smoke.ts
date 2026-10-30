// The owner-run live Bedrock check (T-504; scripts/bedrock-smoke.sh). corral's gate never runs it:
// it costs money on the owner's AWS account. Without --yes-metered it prints what it would call
// and exits non-zero with no network and no file read. With --yes-metered it wires one host exactly
// like src/host/main.ts (stripped AWS_* environment, adopted handlers, socket tripwire, the
// named profile's read-only resolver and the guarded fetch) and runs one turn capped at 16 output
// tokens, then prints the reply, the network ledger and Pi's reported cost.
import { homedir } from "node:os";

import { type AwsBinding } from "./aws.ts";
import { AWS_REGION, isAwsRegion } from "./providers.ts";

export const SMOKE_MAX_TOKENS = 16;
const DEFAULT_MODEL = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";
const PROFILE = /^[A-Za-z0-9_.+=,@-]{1,64}$/;

export interface SmokeArgs {
  readonly binding: AwsBinding;
  readonly model: string;
  readonly metered: boolean;
}

export function parseSmokeArgs(argv: readonly string[]): SmokeArgs {
  const value = (flag: string) => {
    const index = argv.indexOf(flag);
    return index === -1 ? undefined : argv[index + 1];
  };
  const profile = value("--aws-profile");
  const region = value("--region");
  const model = value("--model") ?? DEFAULT_MODEL;
  if (!profile || !PROFILE.test(profile)) throw new Error("usage_error: --aws-profile <name> is required");
  if (!region || !isAwsRegion(region)) throw new Error(`usage_error: --region must match ${AWS_REGION.source} in the aws partition`);
  if (!/^[a-z0-9.:-]+$/.test(model) || model.startsWith("us-gov.")) throw new Error("usage_error: --model must be a bare Bedrock catalog ID");
  return { binding: { profile, region }, model, metered: argv.includes("--yes-metered") };
}

export function smokePlan(args: SmokeArgs): string {
  const endpoint = `https://bedrock-runtime.${args.binding.region}.amazonaws.com/model/${encodeURIComponent(args.model)}/converse-stream`;
  return [
    `endpoint: POST ${endpoint}`,
    `aws profile: ${args.binding.profile}`,
    `region: ${args.binding.region}`,
    `model: ${args.model}`,
    `output token cap: ${SMOKE_MAX_TOKENS}`,
    "billing: metered per token to the AWS account of that profile",
  ].join("\n");
}

async function meteredTurn(args: SmokeArgs): Promise<void> {
  const [{ bedrockRoute, createAwsCredentialResolver }, { hostRules }, restrict, smithy, session, auth] = await Promise.all([
    import("./aws.ts"), import("./providers.ts"), import("../host/restrict.ts"), import("../host/smithy.ts"),
    import("../host/session.ts"), import("./index.ts"),
  ]);
  restrict.restrictEnvironment(process.env, "amazon-bedrock");
  smithy.adoptSmithyHandlers();
  const resolver = createAwsCredentialResolver(args.binding, homedir());
  const rules = hostRules("amazon-bedrock", bedrockRoute(args.binding, args.model, await resolver.profile()));
  const network = restrict.guardedFetch(globalThis.fetch.bind(globalThis), undefined, "amazon-bedrock", rules);
  smithy.installSocketTripwire((destination) => network.refuse?.(destination) ?? new restrict.NetworkRefused(destination));
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  await resolver.get();
  const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
  const models = await ModelRuntime.create({
    credentials: auth.default.credentials(session.authContext("amazon-bedrock")), modelsPath: null,
    refreshOnCreate: false, allowModelNetwork: false,
  });
  session.restrictProvider(models, "amazon-bedrock", network.fetch as typeof globalThis.fetch, undefined,
    { binding: args.binding, resolver });
  const model = models.getModel("amazon-bedrock", args.model);
  if (!model) throw new Error(`model_unavailable: amazon-bedrock/${args.model} is not in Pi's catalog`);
  const reply = await models.complete(model, {
    messages: [{ role: "user", content: "Reply with the single word OK.", timestamp: Date.now() }],
  } as never, { maxTokens: SMOKE_MAX_TOKENS, maxRetries: 0 } as never);
  const text = reply.content.flatMap((block) => block.type === "text" ? [block.text] : []).join("");
  process.stdout.write(`reply: ${reply.stopReason === "error" ? `error (${reply.errorMessage ?? "unknown"})` : text}\n`);
  process.stdout.write(`ledger: ${JSON.stringify(network.ledger())}\n`);
  process.stdout.write(`tokens: input ${reply.usage.input}, output ${reply.usage.output}; ` +
    `reported cost: $${reply.usage.cost.total.toFixed(6)}\n`);
  process.exit(reply.stopReason === "error" ? 1 : 0);
}

export function runSmoke(argv: readonly string[]): void {
  let args: SmokeArgs;
  try {
    args = parseSmokeArgs(argv);
  } catch (error) {
    process.stderr.write(`bedrock-smoke: ${(error as Error).message}\n`);
    process.exit(2);
  }
  process.stdout.write(`${smokePlan(args)}\n`);
  if (!args.metered) {
    process.stdout.write("not sent: pass --yes-metered to send this one metered request\n");
    process.exit(3);
  }
  meteredTurn(args).catch((error: unknown) => {
    process.stderr.write(`bedrock-smoke failed: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  });
}

if (import.meta.main) runSmoke(process.argv.slice(2));
