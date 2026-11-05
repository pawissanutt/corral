import { expect, test } from "bun:test";
import type { Message, ToolResultMessage } from "@earendil-works/pi-ai";

import { buildPromptBlocks } from "../src/claude/prompt.ts";
import { claudeEnvironment } from "../src/claude/stream.ts";

const user = (content: string) => ({ role: "user" as const, content, timestamp: 0 });
const result = (content: ToolResultMessage["content"]): ToolResultMessage => ({
  role: "toolResult", toolCallId: "read-1", toolName: "read", content, isError: false, timestamp: 0,
});

test("Claude replay caches an unchanged text prefix before its moving suffix, with the SDK's TTL", () => {
  const env = claudeEnvironment({ CLAUDE_CODE_PROMPT_CACHE_TTL: "1h" }, "synthetic", "/private");
  const cases: Message[][] = [
    [user("inspect"), result([{ type: "text", text: "file contents" }])],
    [user("inspect"), result([{ type: "text", text: "caption" },
      { type: "image", mimeType: "image/png", data: "aW1hZ2U=" }])],
    [user("inspect"), result([{ type: "image", mimeType: "image/png", data: "aW1hZ2U=" }])],
    [user("inspect"), result([{ type: "text", text: "" }])],
  ];
  for (const history of cases) {
    const first = buildPromptBlocks([...history, user("first question")], new Map());
    const next = buildPromptBlocks([...history, user("first question"),
      result([{ type: "text", text: "new result" }]), user("second question")], new Map());
    const markers = first.flatMap((block, index) => "cache_control" in block ? [index] : []);
    expect(markers).toHaveLength(1);
    const at = markers[0];
    if (at === undefined) throw new Error("history breakpoint missing");
    expect(first[at]).toMatchObject({ type: "text", cache_control: { type: "ephemeral", ttl: "5m" } });
    expect(env.CLAUDE_CODE_PROMPT_CACHE_TTL).toBe("5m");
    // Marker metadata may move; the content prefix at the old write position must not.
    const plain = (blocks: typeof first) => blocks.map((block) => block.type === "text"
      ? { type: block.type, text: block.text } : block);
    expect(plain(next.slice(0, at + 1))).toEqual(plain(first.slice(0, at + 1)));
    expect(first.slice(0, at + 1).some((block) => block.type === "text" && block.text.includes("</conversation_history>")))
      .toBe(false);
    expect(first.at(-1)).toEqual({ type: "text", text: "first question" });
  }
  // A cold request has no historical prefix to mark.
  expect(buildPromptBlocks([user("hello")], new Map()).some((block) => "cache_control" in block)).toBe(false);
});
