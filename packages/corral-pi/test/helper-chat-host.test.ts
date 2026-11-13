import { expect, test } from "bun:test";
import { AssistantMessageComponent, UserMessageComponent } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { HelperChat, promptLabel } from "../src/ui/helper-chat.ts";
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
    // The prompt is the transcript's own user bubble with the conversation's label where the owner's `you` stands:
    // the session's chain (it registers `userLabel` behind a wrapper) followed by `promptLabel`.
    const user = new UserMessageComponent(helper.prompt, presentation.markdownTheme, presentation.outputPad,
      [...presentation.markdownTransformers, promptLabel("prompt")]);
    const owner = new UserMessageComponent(helper.prompt, presentation.markdownTheme, presentation.outputPad, presentation.markdownTransformers);
    const assistant = new AssistantMessageComponent({ content: [{ type: "text", text }], stopReason: "stop" }, true,
      presentation.markdownTheme, "", presentation.outputPad, presentation.markdownTransformers);
    for (const width of [40, 80, 120]) {
      const rendered = chat.renderHelper(helper, width);
      expect(rendered).toEqual([...user.render(width), ...assistant.render(width)]);
      const ours = stripTerminalSequences(rendered[0] ?? ""), theirs = stripTerminalSequences(owner.render(width)[0] ?? "");
      expect(theirs.trimStart().startsWith("you Show the flow")).toBeTrue();
      expect(ours.trimStart().startsWith("prompt Show the flow")).toBeTrue();
      expect(ours.replace("prompt Show", "you Show").trimEnd()).toBe(theirs.trimEnd());
      expect(visibleWidth(rendered[0] ?? "")).toBe(visibleWidth(owner.render(width)[0] ?? ""));
    }
    chat.invalidate();
    expect(chat.renderHelper(helper, 80)).toEqual([...user.render(80), ...assistant.render(80)]);
    expect(streams).toBe(0); expect(session.messages).toHaveLength(0);
    expect(session.sessionManager.getLeafId()).toBe(leaf); expect(session.model).toBe(model);
    expect(h.host.sessionId()).toBe(h.bound); expect(h.screen().text()).toContain("Draft stays");
  } finally { await h.stop(); }
}, 30_000);
