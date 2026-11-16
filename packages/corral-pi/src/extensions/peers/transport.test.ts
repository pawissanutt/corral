import { expect, test } from "bun:test";
import type { CorralOp } from "../../roles/types.ts";
import { CorralClient, type CorralCall, type CorralReceipt } from "../../tools/corral.ts";
import { createPeerTransport } from "./transport.ts";

class Client extends CorralClient {
  readonly calls: { op: CorralOp; payload: Record<string, unknown> }[] = [];
  roster: unknown = { self: "T-001_R1", peers: [
    { key: "T-002_R1", role: "worker", title: "Second task", state: "live" },
    { key: "pm", role: "pm", state: "offline" },
  ] };
  delivery: unknown = { state: "delivered", id: "T-002_R1#7" };
  rejected = false;
  constructor() {
    super({ corral: "unused", root: "/unused", hostCap: "unused", sessionId: () => "session",
      identity: { store: "store", project: "project", role: "worker", task: "T-001", run: "R1", generation: 1 } });
  }
  override async call(op: CorralOp, call: CorralCall): Promise<CorralReceipt> {
    this.calls.push({ op, payload: call.payload });
    return { version: 1, request_id: call.requestId, op, state: this.rejected ? "rejected" : "committed",
      result: this.rejected ? { code: "container_not_found", reason: "Session is unbound" }
        : op === "peer_roster" ? this.roster : this.delivery };
  }
}

test("peer transport maps deliveries and refreshes its roster at five seconds", async () => {
  let now = 1_000;
  const clock = Date.now;
  Date.now = () => now;
  try {
    const client = new Client();
    const transport = createPeerTransport(client);
    expect(transport.self()).toBeUndefined();
    const peers = await transport.roster();
    expect(peers).toEqual([
      { key: "T-002_R1", role: "worker", title: "Second task", state: "live" },
      { key: "pm", role: "pm", state: "offline" },
    ]);
    expect(transport.self()).toBe("T-001_R1");
    now = 5_999;
    expect(await transport.roster()).toBe(peers);
    expect(client.calls.filter(call => call.op === "peer_roster")).toHaveLength(1);
    now = 6_000;
    client.roster = { self: "T-001_R2", peers: [] };
    expect(await transport.roster()).toEqual([]);
    expect(transport.self()).toBe("T-001_R2");
    expect(client.calls.filter(call => call.op === "peer_roster")).toHaveLength(2);
    for (const delivery of [
      { state: "delivered", id: "T-002_R1#7" },
      { state: "queued", id: "T-002_R1#8", detail: "host_unreachable" },
      { state: "undeliverable", detail: "recipient_closed" },
    ]) {
      client.delivery = delivery;
      expect(await transport.send("T-002_R1", "Useful context")).toEqual(delivery);
      expect(client.calls.at(-1)).toEqual({ op: "peer_send", payload: { to: "T-002_R1", text: "Useful context" } });
    }
    client.rejected = true;
    now = 11_000;
    expect(await transport.roster()).toEqual([]);
    expect(transport.self()).toBeUndefined();
  } finally { Date.now = clock; }
});
