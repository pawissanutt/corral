import { describe, expect, test } from "bun:test";
import { parseOwnerAttachments } from "../src/host/owner-attachments.ts";

describe("owner attachments", () => {
  test("accepts more than four distinct attachment labels", () => {
    const items = Array.from({ length: 10 }, (_, index) => ({
      id: `a${index + 1}`, kind: "link", name: "reference", url: "https://example.com/",
    }));
    expect(parseOwnerAttachments(items)).toEqual(items);
    expect(() => parseOwnerAttachments([...items, items[0]])).toThrow();
    expect(() => parseOwnerAttachments([{ ...items[0], id: "a0" }])).toThrow();
  });
});
