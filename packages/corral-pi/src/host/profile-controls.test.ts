import { expect, test } from "bun:test";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { installProfileControls, nextProfile, PROFILE_KEY, type ProfileChoice, type ProfileController } from "./profile-controls.ts";

const choice = (name: string, connection: ProfileChoice["connection"], available = true, reason?: string): ProfileChoice =>
  ({ name, label: name, model: `anthropic/${name}`, connection, available, ...(reason ? { reason } : {}) });
const PROFILES = [
  choice("pi-sonnet", "anthropic-subscription"),
  choice("pi-opus-api", "anthropic-api"),
  choice("pi-sol-api", "openai-api", false, "no OpenAI API key"),
];

function harness(options: { key?: null; select?: (name: string) => Promise<void>; blocked?: string; answer?: (rows: string[]) => string | undefined;
  list?: readonly ProfileChoice[]; confirm?: () => boolean } = {}) {
  let confirms = 0;
  let current: string | undefined;
  const selected: string[] = [], notices: [string, string | undefined][] = [], shortcuts = new Map<string, (ctx: ExtensionContext) => Promise<void>>();
  let command!: (args: string, ctx: ExtensionContext) => Promise<void>;
  const pi = {
    registerCommand: (_name: string, spec: { handler: typeof command }) => { command = spec.handler; },
    registerShortcut: (key: string, spec: { handler: (ctx: ExtensionContext) => Promise<void> }) => { shortcuts.set(key, spec.handler); },
  } as unknown as ExtensionAPI;
  const controller: ProfileController = {
    list: async () => options.list ?? PROFILES,
    current: () => current,
    blocked: () => options.blocked,
    select: async name => { await options.select?.(name); selected.push(name); current = name; },
  };
  const ctx = { hasUI: true, ui: {
    notify: (text: string, tone?: string) => { notices.push([text, tone]); },
    select: async (_title: string, rows: string[]) => options.answer?.(rows),
    confirm: async () => { confirms++; return options.confirm?.() ?? false; },
  } } as unknown as ExtensionContext;
  const installed = installProfileControls(pi, controller, options.key === null ? { key: null } : {});
  return { installed, selected, notices, ctx, run: (args: string) => command(args, ctx), confirms: () => confirms,
    cycle: () => shortcuts.get(PROFILE_KEY)!(ctx), setCurrent: (name?: string) => { current = name; }, shortcuts };
}

test("a profile that opens a new session is confirmed first from the picker and the key; declining changes nothing", async () => {
  const fresh = { ...choice("pi-opus-api", "anthropic-api"), newSession: true, allowanceMicros: 50_000_000 };
  const list = [choice("pi-sonnet", "anthropic-subscription"), fresh];
  const declined = harness({ list, answer: rows => rows[1], confirm: () => false });
  declined.setCurrent("pi-sonnet");
  await declined.run("");
  await declined.cycle();
  await declined.run("pi-opus-api");
  expect(declined.confirms()).toBe(3);
  expect(declined.selected).toEqual([]);
  expect(declined.notices).toEqual([]);
  const accepted = harness({ list, confirm: () => true });
  accepted.setCurrent("pi-sonnet");
  await accepted.cycle();
  expect(accepted.confirms()).toBe(1);
  expect(accepted.selected).toEqual(["pi-opus-api"]);
  const plain = harness({ list: PROFILES });
  await plain.cycle();
  expect(plain.confirms()).toBe(0);
  expect(plain.selected).toEqual(["pi-sonnet"]);
});

test("cancelling /profiles changes nothing and an unavailable row never switches", async () => {
  const cancelled = harness({ answer: () => undefined });
  await cancelled.run("");
  expect(cancelled.selected).toEqual([]); expect(cancelled.notices).toEqual([]);
  const unavailable = harness({ answer: rows => rows[2] });
  await unavailable.run("");
  await unavailable.run("pi-sol-api");
  expect(unavailable.selected).toEqual([]);
  expect(unavailable.notices.every(([text, tone]) => tone === "warning" && text.includes("unavailable: no OpenAI API key"))).toBe(true);
});

test("/profiles <name> selects exactly that profile and refuses unknown names", async () => {
  const h = harness();
  await h.run("pi-opus");
  expect(h.selected).toEqual([]);
  await h.run("pi-opus-api");
  expect(h.selected).toEqual(["pi-opus-api"]);
  expect(h.notices.at(-1)?.[1]).toBe("info");
});

test("the key cycles available profiles from the default, skipping unavailable ones and wrapping", async () => {
  expect(nextProfile(PROFILES, undefined)?.name).toBe("pi-sonnet");
  expect(nextProfile(PROFILES, "pi-opus-api")?.name).toBe("pi-sonnet");
  expect(nextProfile([PROFILES[0]!, PROFILES[2]!], "pi-sonnet")).toBeUndefined();
  const h = harness();
  expect(h.installed.key).toBe(PROFILE_KEY);
  await h.cycle(); await h.cycle(); await h.cycle();
  expect(h.selected).toEqual(["pi-sonnet", "pi-opus-api", "pi-sonnet"]);
  expect(harness({ key: null }).shortcuts.size).toBe(0);
});

test("a blocked or refused switch is an owner notice and keeps the current profile", async () => {
  const blocked = harness({ blocked: "the agent is working; switch when it is idle" });
  await blocked.cycle();
  expect(blocked.selected).toEqual([]);
  expect(blocked.notices[0]?.[1]).toBe("warning");
  expect(blocked.notices[0]?.[0]).toContain("the agent is working");
  const refused = harness({ select: async () => { throw new Error("paid_lineage: switching would reset the paid run"); } });
  refused.setCurrent("pi-sonnet");
  await refused.cycle();
  expect(refused.selected).toEqual([]);
  expect(refused.notices[0]?.[1]).toBe("error");
  expect(refused.notices[0]?.[0]).toContain("paid_lineage");
});
