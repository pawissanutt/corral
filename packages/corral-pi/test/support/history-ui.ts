import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import type { Component } from "@earendil-works/pi-tui";
import { PopupList } from "../../src/ui/popup-list.ts";

const ANSI = /\x1b\[[0-9;:?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b_[^\x1b]*\x1b\\/g;
/** Lets the navigator's pending reads (fake, promise-based) settle between keys. */
function settle(): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setTimeout(resolve, 0);
  return promise;
}

/**
 * A scripted display API for the History navigator, driving its real components by keys. The list popup takes
 * `selections`: the first is the kind (Tab presses: 0 Tasks, 1 Assistants), each later number picks that row (↓
 * presses, then Enter: a task opens its runs, a run or assistant is chosen), a string is typed into the filter and
 * entered (a task search); `undefined` or none left closes it. Each pager takes keys from `actions` until it closes
 * (none left: Esc). `frames` holds every drawn frame at two widths, plain. `canInteract` answers as the rich host
 * does: false while any of these overlays is mounted (Pi hides an overlay before its promise resolves).
 */
export function historyUI(selections: (number | string | undefined)[], actions: string[] = ["\x1b"]) {
  const frames: string[] = [];
  let draft = "unsent draft", mounted = 0;
  const ui = {
    getEditorText: () => draft,
    setEditorText: (text: string) => { draft = text; },
    custom: async (factory: (tui: unknown, theme: unknown, keys: unknown, done: (value: unknown) => void) => Component) => {
      let result: unknown, closed = false;
      const tui = { terminal: { rows: 40, columns: 120 }, requestRender() {} };
      mounted++;
      const component = factory(tui, {}, {}, value => { if (!closed) mounted--; result = value; closed = true; });
      const draw = () => { if (!closed) for (const width of [48, 120]) frames.push(component.render(width).join("\n").replace(ANSI, "")); };
      if (component instanceof PopupList) {
        await settle(); draw();
        let kind = true;
        while (!closed) {
          const pick = selections.shift();
          if (pick === undefined) break;
          if (typeof pick === "string") { for (const char of pick) component.handleInput(char); component.handleInput("\r"); }
          else {
            for (let i = 0; i < pick; i++) component.handleInput(kind ? "\t" : "\x1b[B");
            if (!kind) component.handleInput("\r");
          }
          kind = false;
          await settle(); draw();
        }
        // Esc leaves a task's runs first, then closes the list.
        for (let i = 0; i < 3 && !closed; i++) { component.handleInput("\x1b"); await settle(); }
      } else {
        const press = component.handleInput?.bind(component);
        if (!press) throw new Error("History overlay takes no keys");
        for (let i = 0; i < 20 && !closed; i++) { draw(); press(actions.shift() ?? "\x1b"); }
      }
      return result;
    },
  } as unknown as ExtensionUIContext;
  return { ui, frames, canInteract: () => mounted === 0 };
}
