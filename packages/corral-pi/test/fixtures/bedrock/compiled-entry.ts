// The entry bedrock-compiled.test.ts compiles with scripts/build-pi-host.sh's flags: one fake
// Bedrock turn through handler adoption and pi-ai's Bedrock client (which a compiled host can only
// load through the module adoption registers), then a raw node:https request under the tripwire.
import https from "node:https";

import { installSocketTripwire } from "../../../src/host/smithy.ts";
import { STATIC_DEV, awsHome, bedrockHost, fakeAws, signingKey } from "./fake-aws.ts";

const fake = fakeAws();
const host = await bedrockHost(awsHome(STATIC_DEV), fake.send);
installSocketTripwire((destination) => host.network.refuse!(destination));
const reply = await host.turn();
let raw: string;
try {
  https.request("https://bedrock-runtime.us-west-2.amazonaws.invalid/");
  raw = "opened";
} catch (error) {
  raw = (error as { code?: string }).code ?? String(error);
}
process.stdout.write(`${JSON.stringify({
  stopReason: reply.stopReason, errorMessage: reply.errorMessage ?? null, content: reply.content,
  usage: [reply.usage.input, reply.usage.output, reply.usage.totalTokens],
  requests: fake.requests.map((request) => [request.method, request.url, signingKey(request)]),
  ledger: host.network.ledger(), raw,
})}\n`);
process.exit(0);
