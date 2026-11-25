import { canonical, closedObject, FixturePipe, requireWire } from "./wire.ts";
import type { JsonValue } from "@earendil-works/pi-ai";

const VERSION = "2025-06-18";

// This stdio exchange is relayed to the private shim by the mediator. It only
// verifies the evidence registry; the mediator still owns call IDs and arming.
export async function negotiateEvidence(pipe: FixturePipe, pinnedEvidence: JsonValue[]): Promise<void> {
  async function exchange(request: object) {
    await pipe.send({ type: "mcp", request });
    const frame = closedObject(await pipe.receive(), "type", "response");
    requireWire(frame.type === "mcp");
    return frame.response!;
  }

  const info = await exchange({
    jsonrpc: "2.0", id: 1, method: "initialize", params: {
      protocolVersion: VERSION, capabilities: {}, clientInfo: { name: "corral-synthetic", version: "1" },
    },
  });
  requireWire(canonical(info) === canonical({
    jsonrpc: "2.0", id: 1, result: {
      protocolVersion: VERSION, capabilities: { tools: {} }, serverInfo: { name: "corral-lite-mcp-shim", version: "1" },
    },
  }));

  await pipe.send({ type: "mcp", request: { jsonrpc: "2.0", method: "notifications/initialized" } });
  const list = closedObject(await exchange({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }),
    "jsonrpc", "id", "result");
  const { tools } = closedObject(list.result, "tools");
  requireWire(list.jsonrpc === "2.0" && list.id === 2 && Array.isArray(tools) && tools.length === pinnedEvidence.length);
  // Compare exact descriptors as a multiset, without changing the provider registry's order.
  const expected = pinnedEvidence.map(canonical).sort();
  requireWire(tools.map(canonical).sort().every((tool, index) => tool === expected[index]));
}
