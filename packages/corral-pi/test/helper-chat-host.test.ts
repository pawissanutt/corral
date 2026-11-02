import { expect, test } from "bun:test";
import { AssistantMessageComponent, UserMessageComponent } from "@earendil-works/pi-coding-agent";
import { HelperChat } from "../src/ui/helper-chat.ts";
import type { HelperInspection } from "../src/extensions/subagent/inspection.ts";
import { startPanelHost } from "./support/panel-host.ts";

test("the real InteractiveMode supplies the same TUI, Markdown/Mermaid pipeline and render-only tool definitions", async () => {
  const h = await startPanelHost();
  try {
    for (const character of "Draft stays") h.physical.send(character);
    await h.physical.whenWritten(() => h.screen().text().includes("Draft stays"), "parent draft");
    const session = h.host.session, leaf = session.sessionManager.getLeafId(), model = session.model;
    let streams = 0;
    h.host.runtime.services.modelRuntime.stream = () => { streams++; throw new Error("inspection cannot start a provider"); };
    h.host.runtime.services.modelRuntime.streamSimple = () => { streams++; throw new Error("inspection cannot start a provider"); };
    const presentation = h.chatPresentation();
    expect(h.chatPresentation().tui).toBe(presentation.tui);
    expect(presentation.markdownTransformers.length).toBeGreaterThan(0);
    const read = presentation.renderers("read");
    if (!read) throw new Error("native read renderer unavailable");
    expect("execute" in read).toBe(false);
    const text = "## Native diagram\n\n```mermaid\ngraph TD\n  A[Research] --> B[Answer]\n```";
    const helper: HelperInspection = { request: "display-fixture", generation: 1, kind: "explore", model: "fixture/model", createdAt: 0,
      prompt: "Show the flow", phase: "completed", truncated: false, result: text,
      messages: [{ id: "assistant", kind: "assistant", text, stopReason: "stop" }] };
    const chat = new HelperChat(() => h.chatPresentation().tui, h.work, h.chatPresentation);
    const user = new UserMessageComponent(helper.prompt, presentation.markdownTheme, presentation.outputPad, presentation.markdownTransformers);
    const assistant = new AssistantMessageComponent({ content: [{ type: "text", text }], stopReason: "stop" }, true,
      presentation.markdownTheme, "", presentation.outputPad, presentation.markdownTransformers);
    for (const width of [40, 80, 120]) expect(chat.renderHelper(helper, width)).toEqual([...user.render(width), ...assistant.render(width)]);
    chat.invalidate();
    expect(chat.renderHelper(helper, 80)).toEqual([...user.render(80), ...assistant.render(80)]);
    expect(streams).toBe(0); expect(session.messages).toHaveLength(0);
    expect(session.sessionManager.getLeafId()).toBe(leaf); expect(session.model).toBe(model);
    expect(h.host.sessionId()).toBe(h.bound); expect(h.screen().text()).toContain("Draft stays");
  } finally { await h.stop(); }
}, 30_000);
