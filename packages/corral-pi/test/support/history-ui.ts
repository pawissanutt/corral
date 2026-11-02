import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import type { Component } from "@earendil-works/pi-tui";
export function historyUI(selections: (number | undefined)[], actions: string[] = ["\x1b"]) {
  const frames: string[] = [], notices: string[] = [];
  let draft = "unsent draft";
  const ui = {
    select: async (_title: string, options: string[]) => { const index = selections.shift(); return index === undefined ? undefined : options[index]; },
    input: async () => "",
    notify: (message: string) => { notices.push(message); },
    getEditorText: () => draft,
    setEditorText: (text: string) => { draft = text; },
    custom: async (factory: (tui: unknown, theme: unknown, keys: unknown, done: (value: string | undefined) => void) => Component) => {
      let result: string | undefined;
      const component = factory({ terminal: { rows: 40 }, requestRender() {} }, {}, {}, value => { result = value; });
      for (const width of [48, 120]) frames.push(component.render(width).join("\n"));
      component.handleInput?.(actions.shift() ?? "\x1b");
      return result;
    },
  } as unknown as ExtensionUIContext;
  return { ui, frames, notices };
}
