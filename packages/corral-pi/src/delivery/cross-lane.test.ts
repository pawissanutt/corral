// Cross-lane fixture for the inbox adapter: the real corral binary dispatches a pi-native run
// whose fixture installation keeps the package's real delivery module, in the private
// plugin-free herdr server of scripts/test-herdr.sh. `corral reply` reaches the pane host by
// bridge submit_input, the module writes the corral_delivery entry, and corral observes the
// message only from that entry.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { type FixtureInstall, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import {
  EventFollower, corralBinary, generationDir, ok, privateHerdr, privateRoot, run, sha256,
} from "../../test/support/workspace.ts";
import { ENTRY_TYPE } from "./index.ts";

const isolated = privateHerdr();
const herdrRoot = privateRoot() ?? "";
let corral = "";
let workspace = "";
let assets: FixtureInstall | undefined;
let events: EventFollower | undefined;
const workspaces: string[] = [];

const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (task: string): any => JSON.parse(cli("show", task, "--json"));

beforeAll(() => {
  if (!isolated) {
    console.log("delivery cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  workspace = mkdtempSync(join(herdrRoot, "pi-delivery-"));
  mkdirSync(join(workspace, "app"));
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", join(workspace, "app"), ...argv]);
  writeFileSync(join(workspace, "app/README.md"), "# app\n");
  ok(["git", "-C", join(workspace, "app"), "add", "README.md"]);
  ok(["git", "-C", join(workspace, "app"), "commit", "-q", "-m", "initial commit"]);
  cli("init");
  assets = buildFixtureInstall({ parent: workspace, omit: ["delivery"] });
  const config = join(workspace, ".corral/corral.toml");
  writeFileSync(config, `${readFileSync(config, "utf8")}
[profiles.pi]
harness = "pi-native"
model = "gpt-6.1-sol"
vendor = "openai"
effort = "high"

[agent]
assets_dir = ${JSON.stringify(assets.dir)}
`);
  cli("new", "Pi inbox delivery", "-p", "app", "--ready");
  events = new EventFollower(join(workspace, ".corral/events.jsonl"));
}, 900_000);

afterAll(() => {
  events?.close();
  for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
  if (workspace) {
    rmSync(join(generationDir(workspace, "T-001", "R1", 1), ".."), { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
});

test("corral reply reaches the live pi-native worker by submit_input and is observed only from its corral_delivery entry", async () => {
  if (!isolated) return;
  const bound = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  workspaces.push(show("T-001").runs.at(-1).herdr.workspace_id);
  await bound;
  const delivery = show("T-001").runs.at(-1).delivery;
  expect(delivery).toBe("inbox");

  const ended = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001");
  cli("reply", "T-001", "Reply only INBOX.");
  await ended;
  cli("sync");

  const inbox = join(workspace, ".corral/inbox/T-001_R1");
  const envelopes = readdirSync(join(inbox, "envelopes")).filter((name) => name.endsWith(".json"))
    .map((name) => JSON.parse(readFileSync(join(inbox, "envelopes", name), "utf8")));
  expect(envelopes).toHaveLength(1);
  const envelope = envelopes[0];
  expect(envelope.adapter).toBe("pi_native");
  expect(envelope.binding.endpoint).toBe(join(generationDir(workspace, "T-001", "R1", 1), "host.sock"));
  expect(existsSync(join(inbox, "000001.receipt.observed.json"))).toBeTrue();
  const receipt = JSON.parse(readFileSync(join(inbox, "000001.receipt.observed.json"), "utf8"));
  expect(receipt.via).toBe("pi_native");

  const session = readFileSync(envelope.binding.transcript, "utf8").trim().split("\n").map((line) => JSON.parse(line));
  const written = session.filter((entry) => entry.customType === ENTRY_TYPE);
  expect(written).toHaveLength(1);
  expect(written[0].data.idempotency_key).toBe(envelope.idempotency_key.replace("@env", ":env"));
  expect(written[0].data.text_sha256).toBe(envelope.payload_sha256);
  expect(written[0].data.origin).toBe("corral-inbox");
  expect(sha256(readFileSync(envelope.payload_path, "utf8"))).toBe(envelope.payload_sha256);
  expect(receipt.detail.entry_id).toBe(written[0].id);
  const turns = show("T-001").runs.at(-1).agent_host.turns as { origin: { kind: string; idempotency_key?: string } }[];
  expect(turns.at(-1)?.origin).toEqual({ kind: "bridge", idempotency_key: written[0].data.idempotency_key });
}, 300_000);
