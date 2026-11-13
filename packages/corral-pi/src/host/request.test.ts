import { expect, test } from "bun:test";
import { RequestContext } from "./request.ts";

test("refused notice marks retain the transient notification, while accepted marks do not duplicate it", async () => {
  const hooks: { start?: (event: never, context: never) => unknown } = {};
  const notices: string[] = [];
  let accepts = false;
  const request = new RequestContext({ restore: async () => false } as never, () => {}, { add: () => accepts });
  request.extend({
    on(name: string, handler: (event: never, context: never) => unknown) {
      if (name === "session_start") hooks.start = handler;
      return () => {};
    },
  } as never);
  if (!hooks.start) throw new Error("session-start handler was not installed");
  await hooks.start(undefined as never, { ui: { notify: (text: string) => { notices.push(text); } } } as never);
  request.notify("refused");
  accepts = true;
  request.notify("recorded");
  request.notify("transient-only", false);
  expect(notices).toEqual(["refused", "transient-only"]);
});
