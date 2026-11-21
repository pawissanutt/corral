import { expect, test } from "bun:test";
import { convertToLlm, SessionManager } from "@earendil-works/pi-coding-agent";
import { authContext } from "../src/host/session.ts";
import { createModelCatalog } from "../src/host/catalog.ts";
import auth from "./fixtures/auth/index.ts";
import { fakeCodexTransport, requestBody } from "./fixtures/auth/fake-transport.ts";

test("Codex sends persistent instructions in the cacheable prefix before and after compaction", async () => {
  const provider = "openai-codex";
  const credentials = auth.credentials(authContext(provider));
  const runtime = await createModelCatalog(credentials, { providers: [provider], modelsPath: null });
  const model = runtime.getModel(provider, "gpt-6.2-sol")!;
  const credential = await credentials.read(provider);
  if (credential?.type !== "oauth") throw new Error("missing fixture credential");
  const journal = SessionManager.inMemory();
  const guide = "Ask the owner before editing the release manifest.";
  journal.appendMessage({ role: "system", content: guide, timestamp: 1 });
  journal.appendMessage({ role: "user", content: "Read the old checklist.", timestamp: 2 });
  const retained = journal.appendMessage({ role: "user", content: "Continue the build.", timestamp: 3 });
  const transport = fakeCodexTransport();
  for (const compacted of [false, true]) {
    if (compacted) journal.appendCompaction("Checklist reviewed.", retained, 100);
    let body: unknown;
    const response = await runtime.streamSimple(model, { messages: convertToLlm(journal.buildSessionContext().messages) }, {
      apiKey: credential.access, transport: "sse", maxRetries: 0,
      fetch(url, init) { body = requestBody(init); return transport.send(url, init); },
    }).result();
    expect(response.stopReason).toBe("stop");
    if (!body || typeof body !== "object" || !("input" in body) || !Array.isArray(body.input)) throw new Error("missing Codex input");
    expect(body.input[0]).toMatchObject({ role: "developer", content: guide });
    const wire = JSON.stringify(body);
    expect(wire.split(guide)).toHaveLength(2);
    if (compacted) {
      expect(wire).toContain("Checklist reviewed.");
      expect(wire).not.toContain("Read the old checklist.");
    }
  }
});
