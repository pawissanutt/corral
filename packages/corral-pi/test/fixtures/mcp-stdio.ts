import { createInterface } from "node:readline";
import { z } from "zod";

const messageSchema = z.object({
  id: z.union([z.string(), z.number()]).optional(), method: z.string(),
  params: z.record(z.string(), z.unknown()).optional(),
});
const lines = createInterface({ input: process.stdin });
for await (const line of lines) {
  const message = messageSchema.parse(JSON.parse(line));
  if (message.id === undefined) continue;
  let result: unknown;
  switch (message.method) {
    case "initialize":
      result = { protocolVersion: "2025-11-25", capabilities: { tools: {} }, serverInfo: { name: "stdio-fixture", version: "1" } };
      break;
    case "tools/list":
      result = { tools: [{ name: "lookup", inputSchema: { type: "object", properties: {} } }] };
      break;
    case "tools/call":
      result = { content: [{ type: "text", text: "stdio fixture" }] };
      break;
    default:
      throw new Error(`unexpected fixture MCP method: ${message.method}`);
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result })}\n`);
}
