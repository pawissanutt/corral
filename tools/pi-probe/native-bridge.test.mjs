import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createBridge, connectBridge } from "./native-bridge.mjs";

test("authenticated scope, generation, reconnect and stable payload receipts", async () => {
  const root = mkdtempSync(join(tmpdir(), "pi-bridge-test-"));
  let generation = 1;
  let mutations = 0;
  const scope = { store: "fixture", native_session_id: "one" };
  const bridge = await createBridge({ root, capability: "synthetic-capability", scope,
    generation: () => generation, receipt() {},
    async receive(kind) {
      if (kind === "mutate") { mutations++; bridge.emit({ event: "committed" }); }
      return { mutations };
    } });
  const identity = { path: bridge.path, capability: "synthetic-capability", scope, generation };
  const client = await connectBridge(identity);
  const second = await connectBridge(identity);
  try {
    assert.equal((await client.request("mutate", {}, { capability: "wrong" })).error, "foreign_session_scope");
    assert.equal((await client.request("mutate", {}, { scope: { ...scope, store: "foreign" } })).error, "foreign_session_scope");
    assert.equal((await client.request("mutate", {}, { generation: 0 })).error, "stale_generation");
    assert.equal(mutations, 0);
    await second.request("state");
    const committed = second.waitFor("committed");
    client.sendAndClose("mutate", {}, "lost-ack");
    await committed;
    const replay = await second.request("mutate", {}, { id: "lost-ack" });
    assert.deepEqual(replay.data, { mutations: 1 });
    assert.equal((await second.request("mutate", { different: true }, { id: "lost-ack" })).error,
      "conflicting_request_payload");
    assert.equal(mutations, 1);
    generation++;
    scope.native_session_id = "two";
    assert.equal((await second.request("state")).error, "foreign_session_scope");
    second.setSession(generation, "two");
    assert.deepEqual((await second.request("state")).data, { mutations: 1 });
  } finally {
    client.close(); second.close(); await bridge.close();
    rmSync(root, { recursive: true });
  }
});

test("in-flight duplicates share one execution and receipt capacity precedes effects", async () => {
  const root = mkdtempSync(join(tmpdir(), "pi-bridge-test-"));
  let mutations = 0;
  const bridge = await createBridge({ root, capability: "test", scope: {},
    generation: () => 1, receipt() {},
    async receive() { mutations++; return { mutations }; } });
  const client = await connectBridge({ path: bridge.path, capability: "test", scope: {}, generation: 1 });
  try {
    const responses = await Promise.all([client.request("mutate", {}, { id: "same" }),
      client.request("mutate", {}, { id: "same" })]);
    assert.deepEqual(responses[0].data, responses[1].data);
    assert.equal(mutations, 1);
    for (let index = 1; index < 64; index++) await client.request("mutate");
    assert.equal((await client.request("mutate")).error, "fixture_receipt_limit");
    assert.equal(mutations, 64);
  } finally {
    client.close(); await bridge.close(); rmSync(root, { recursive: true });
  }
});
