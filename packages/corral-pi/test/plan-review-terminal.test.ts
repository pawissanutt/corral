// The terminal's plan review through a real InteractiveMode and editor: only the provider is scripted.
import { expect, test } from "bun:test";
import { writeFileSync } from "node:fs";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { startPanelHost, type PanelHost } from "./support/panel-host.ts";

const type = (h: PanelHost, text: string) => { for (const char of text) h.physical.send(char); };
const frame = async (h: PanelHost, text: string) => {
  try { await h.physical.whenWritten(() => h.screen().text().includes(text), text); }
  catch (error) { throw new Error(`${String(error)}\n${h.screen().text()}`); }
};

test("the terminal plan review keeps the keys after the owner changes the model from it", async () => {
  const h = await startPanelHost(undefined, { role: "assistant" });
  try {
    const plan = h.host.planMode, session = h.host.session;
    if (!plan) throw new Error("the assistant role has no plan mode");
    // Given a submitted plan, whose review opens by itself once the turn has ended:
    await plan.enter();
    writeFileSync(plan.path, "# Add the flag\n\n1. Parse --version.\n");
    fixtureTransport().script([{ toolCalls: [{ callId: "submit", name: "plan_submit", arguments: {} }] }, { text: "Submitted." }]);
    await session.prompt("Plan a --version flag", { source: "extension" });
    await frame(h, "Plan review · Add the flag");
    const original = session.model;
    const target = (await h.host.runtime.services.modelRuntime.getAvailable()).find(model => model.id !== original?.id && model.reasoning);
    if (!target) throw new Error("the fixture needs a second model");
    // When the owner changes the model from the review:
    h.physical.send("m");
    await frame(h, "Only showing models from configured providers");
    type(h, target.id);
    h.physical.send("\r");
    await h.physical.whenWritten(() => session.model?.id === target.id, "the model change");
    await frame(h, `revision 1 · ${target.provider}/${target.id}`);
    // Then the reopened review has the keys: `e` executes the plan, on that model.
    fixtureTransport().script([{ text: "Executing." }]);
    h.physical.send("e");
    await h.physical.whenWritten(() => plan.phase === "off", "the plan to start");
    expect(plan.approved?.title).toBe("Add the flag");
    expect(session.model?.id).toBe(target.id);
  } finally { await h.stop(); }
}, 30_000);
