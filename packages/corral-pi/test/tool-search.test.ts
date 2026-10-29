import { expect, test } from "bun:test";
import { Type } from "typebox";
import { toolSearch } from "../src/host/tool-search.ts";

test("cached search filters forbidden metadata without MCP, activation or a connection", async () => {
  // Given only a metadata getter: no client, transport, activation API or codemode exists.
  let reads = 0;
  const search = toolSearch({
    getAllTools() {
      reads++;
      return ["approved", "forbidden"].map((name) => ({
        name, description: "fixture tool", parameters: Type.Object({}), exposure: "deferred" as const,
        sourceInfo: { source: "extension", path: "/fixture", scope: "temporary" as const, origin: "top-level" as const },
      }));
    },
  }, (name) => name === "approved");
  // When searching, then only the authorized cached schema is returned, using one local read.
  const result = await search.execute("cached", { query: "fixture", limit: 5 });
  expect(reads).toBe(1);
  const first = result.content[0];
  expect(JSON.parse(first?.type === "text" ? first.text : "null").tools.map((tool: { name: string }) => tool.name)).toEqual(["approved"]);
});
