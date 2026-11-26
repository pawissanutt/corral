import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import fixtureAuth from "../../test/fixtures/auth/index.ts";
import { fakeCodexTransport, requestBody } from "../../test/fixtures/auth/fake-transport.ts";
import { PACKAGE_ROOT } from "../../test/support/fixture-install.ts";
import type { AuthModule } from "../auth/types.ts";
import { EventLog } from "../bridge/events.ts";
import { assistantRole } from "../roles/assistant/index.ts";
import { CorralClient } from "../tools/corral.ts";
import { APPROVAL_CHOICES as CHOICE, PendingQuestions } from "./pending-questions.ts";
import { ToolApprovals } from "./tool-approvals.ts";
import { ImageGeneration } from "../extensions/image/tool.ts";
import { authorizeImageDestination } from "./image-authorization.ts";
import { guardedFetch } from "./restrict.ts";
import { HostSession } from "./session.ts";

const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
const linuxTest = test.skipIf(process.platform !== "linux");

async function imageHost(run: (f: {
  host: HostSession; outside: string; path: string; requests: unknown[];
  draw: (prompt?: string, path?: string) => Promise<{ isError: boolean; text: string }>;
  answer: (option: string) => Promise<void>;
}) => Promise<void>, duringImageRequest?: (outside: string) => void, duringCredentialRead?: (outside: string) => void) {
  const base = mkdtempSync(join(tmpdir(), "corral-image-approval-"));
  let host: HostSession | undefined;
  try {
    const cwd = join(base, "checkout"), outside = join(base, "outside");
    mkdirSync(cwd);
    mkdirSync(outside);
    const path = join(outside, "dot.png");
    const transport = fakeCodexTransport(), requests: unknown[] = [];
    const auth: AuthModule = { provider: "openai-codex", providers: ["openai-codex", "openai"],
      credentials: ctx => ctx.provider === "openai-codex" ? fixtureAuth.credentials(ctx) : {
        read: async () => { duringCredentialRead?.(outside); return { type: "api_key", key: "sk-SYNTHETIC-image" }; },
        list: async () => [{ providerId: "openai", type: "api_key" }],
        modify: async () => { throw new Error("fixture is read-only"); }, delete: async () => {},
      },
      transport: ({ provider }) => provider === "openai-codex" ? transport.send : async (url, init) => {
        expect(url).toBe("https://api.openai.com/v1/images/generations");
        requests.push(requestBody(init));
        duringImageRequest?.(outside);
        return Response.json({ data: [{ b64_json: png }] });
      },
    };
    const identity = { store: "st-demo", project: "", role: "assistant" as const, assistant: "A-1" };
    const live = host = await HostSession.create({ sdk, identity, cwd, agentDir: join(base, "agent"),
      model: "gpt-6.1-sol", ui: "plain", modules: { role: assistantRole(PACKAGE_ROOT), auth },
      capabilities: { catalog: 8, enabled: [], hosted_tools: { image_gen: { provider: "openai", model: "gpt-image-1" } } },
      client: new CorralClient({ corral: "", root: base, cwd, identity }), events: new EventLog(),
      network: guardedFetch(transport.send), sessionManager: sdk.SessionManager.create(cwd, join(base, "sessions")) });
    await live.session.bindExtensions({});
    await live.startLocal();
    let count = 0;
    await run({ host: live, outside, path, requests,
      async draw(prompt = "Draw a red dot", destination = path) {
        const callId = `draw-${++count}`;
        transport.script([{ toolCalls: [{ callId, name: "image_gen", arguments: { prompt, path: destination } }] }, { text: "Done." }]);
        await live.session.prompt(`Try ${callId}.`, { source: "extension" });
        const result = live.session.messages.find(message => message.role === "toolResult" && message.toolCallId.split("|")[0] === callId);
        if (result?.role !== "toolResult") throw new Error(`No result for ${callId}`);
        return { isError: result.isError, text: result.content.flatMap(part => part.type === "text" ? [part.text] : []).join("\n") };
      },
      async answer(option) {
        const pending = live.pendingOwnerQuestions();
        expect(pending).toHaveLength(1);
        await live.answerQuestion({ question: pending[0]!.id, option }, async () => {});
        await live.session.waitForIdle();
      },
    });
  } finally {
    try { await host?.close(); } finally { rmSync(base, { recursive: true, force: true }); }
  }
}

linuxTest("image_gen refuses a classification that differs from the prepared destination before creating an approval", async () => {
  const base = mkdtempSync(join(tmpdir(), "corral-image-mismatch-"));
  const root = join(base, "root"), outside = join(base, "outside"), other = join(base, "other");
  mkdirSync(root); mkdirSync(outside); mkdirSync(other);
  let requests = 0;
  const questions = new PendingQuestions({ session: () => "session", entries: () => [], append: () => {},
    changed: () => {}, deliver: async () => false });
  const approvals = new ToolApprovals(questions, root, true);
  const image = new ImageGeneration({ provider: "openai", model: "gpt-image-1" }, {
    auth: { provider: "openai-codex", providers: ["openai"], credentials: () => ({
      read: async () => ({ type: "api_key", key: "sk-fixtureOnly" }), list: async () => [],
      modify: async () => { throw new Error("unexpected write"); }, delete: async () => {},
    }) },
    fetch: async () => { requests++; return Response.json({ data: [{ b64_json: png }] }); },
  }, () => {});
  try {
    const destination = join(outside, "dot.png");
    const tool = image.tool({ root, refusal: () => undefined,
      authorize: (path, prompt, prepared, identity) => authorizeImageDestination(path, prompt, prepared, identity,
        { cwd: root, root }, approvals, () => ({ action: "ask", path: prepared, canonical: join(other, "dot.png") })) });
    await expect(tool.execute("mismatch", { prompt: "dot", path: destination }, undefined, undefined, {} as never))
      .rejects.toThrow("changed since preparation");
    expect(questions.pending()).toEqual([]);
    expect(requests).toBe(0);
    expect(readdirSync(outside)).toEqual([]);
    expect(readdirSync(other)).toEqual([]);
  } finally { image.close(); rmSync(base, { recursive: true, force: true }); }
});

linuxTest("image_gen requests approval before dispatch and allow-once covers only the exact request", async () => {
  await imageHost(async ({ host, outside, path, requests, draw, answer }) => {
    const first = await draw();
    const pending = host.pendingOwnerQuestions();
    expect(pending).toHaveLength(1);
    const [question] = pending;
    expect(first.isError).toBeTrue();
    expect(first.text).toContain(question!.id);
    expect(question?.question).toContain("image_gen");
    expect(question?.question).toContain(path);
    expect(question?.free_text).toBeFalse();
    expect(requests).toEqual([]);
    expect(readdirSync(outside)).toEqual([]);

    await answer(CHOICE.once);
    const changed = await draw("Draw a blue dot");
    expect(changed.isError).toBeTrue();
    expect(changed.text).toContain("Waiting for owner approval");
    expect(requests).toEqual([]);
    expect(readdirSync(outside)).toEqual([]);

    const approved = await draw();
    expect(approved.isError).toBeFalse();
    expect(JSON.parse(approved.text)).toMatchObject({ provider: "openai", path });
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({ prompt: "Draw a red dot" });
    expect(readFileSync(path, "base64")).toBe(png);

    // Remove the file so exclusive publication cannot mask a reused grant.
    rmSync(path);
    expect((await draw()).isError).toBeTrue();
    expect(requests).toHaveLength(1);
    expect(readdirSync(outside)).toEqual([]);
  });
}, 30_000);

linuxTest("image_gen session grants survive sibling publication and cover new images", async () => {
  await imageHost(async ({ host, outside, path, requests, draw }) => {
    const second = join(outside, "second.png"), third = join(outside, "third.png");
    expect((await draw()).isError).toBeTrue();
    const firstQuestion = host.pendingOwnerQuestions()[0]!;
    expect((await draw("Draw another dot", second)).isError).toBeTrue();
    const questions = host.pendingOwnerQuestions();
    expect(questions).toHaveLength(2);
    expect(firstQuestion.question).toContain("new images in this directory");
    for (const question of questions) {
      await host.answerQuestion({ question: question.id, option: CHOICE.session }, async () => {});
      await host.session.waitForIdle();
      if (question.id === firstQuestion.id) expect((await draw()).isError).toBeFalse();
    }
    expect((await draw("Draw another dot", second)).isError).toBeFalse();
    expect((await draw("Draw a third dot", third)).isError).toBeFalse();
    expect(host.pendingOwnerQuestions()).toEqual([]);
    expect(requests).toHaveLength(3);
    expect(readFileSync(path, "base64")).toBe(png);
    expect(readFileSync(second, "base64")).toBe(png);
    expect(readFileSync(third, "base64")).toBe(png);
    renameSync(outside, `${outside}-moved`);
    mkdirSync(outside);
    expect((await draw("Draw a fourth dot", join(outside, "fourth.png"))).text).toContain("Waiting for owner approval");
    expect(requests).toHaveLength(3);
  });
}, 30_000);

linuxTest("image_gen treats child ownership declarations as literal paths", async () => {
  await imageHost(async ({ host, outside, requests }) => {
    const cwd = join(dirname(outside), "checkout");
    mkdirSync(join(cwd, "assets"));
    const binding = host as unknown as { boundImageTool(child: { kind: "slice"; owns: string[] }): sdk.ToolDefinition };
    const tool = binding.boundImageTool({ kind: "slice", owns: ["@assets"] });
    await expect(tool.execute("image", { prompt: "dot", path: "assets/dot.png" }, undefined, undefined, {} as never))
      .rejects.toThrow("outside child ownership");
    expect(requests).toEqual([]);
    expect(readdirSync(join(cwd, "assets"))).toEqual([]);
  });
});

linuxTest("image_gen never sends or saves an unanswered or denied request", async () => {
  await imageHost(async ({ host, outside, requests, draw, answer }) => {
    const first = await draw();
    expect(first.isError).toBeTrue();
    expect(await host.replyToQuestion(CHOICE.once)).toBeFalse();
    expect(await draw()).toEqual(first);
    expect(requests).toEqual([]);
    expect(readdirSync(outside)).toEqual([]);

    await answer(CHOICE.deny);
    const denied = await draw();
    expect(denied.isError).toBeTrue();
    expect(denied.text).toContain("denied");
    expect(host.pendingOwnerQuestions()).toEqual([]);
    expect(requests).toEqual([]);
    expect(readdirSync(outside)).toEqual([]);
  });
}, 30_000);

linuxTest("image_gen refuses an approved destination that changed, and never asks for protected state", async () => {
  await imageHost(async ({ host, outside, requests, draw, answer }) => {
    expect((await draw()).isError).toBeTrue();
    await answer(CHOICE.once);
    const moved = `${outside}-moved`;
    renameSync(outside, moved);
    mkdirSync(outside);
    // The canonical string is unchanged, but it now names a different directory.
    const changed = await draw();
    expect(changed.isError).toBeTrue();
    expect(requests).toEqual([]);
    expect(readdirSync(moved)).toEqual([]);
    expect(readdirSync(outside)).toEqual([]);

    const pending = host.pendingOwnerQuestions();
    const state = join(outside, ".corral-next");
    mkdirSync(state);
    const protectedResult = await draw("Draw a red dot", join(state, "dot.png"));
    expect(protectedResult.isError).toBeTrue();
    expect(protectedResult.text).toContain("Corral state");
    expect(host.pendingOwnerQuestions()).toEqual(pending);
    expect(requests).toEqual([]);
    expect(readdirSync(state)).toEqual([]);
  });
}, 30_000);

linuxTest("image_gen cancels a stale approval when its destination directory changes", async () => {
  await imageHost(async ({ host, outside, requests, draw }) => {
    expect((await draw()).isError).toBeTrue();
    expect(host.pendingOwnerQuestions()).toHaveLength(1);
    renameSync(outside, `${outside}-moved`);
    mkdirSync(outside);
    expect((await draw()).text).toContain("destination changed");
    expect(host.pendingOwnerQuestions()).toHaveLength(0);
    expect((await draw()).text).toContain("Waiting for owner approval");
    expect(host.pendingOwnerQuestions()).toHaveLength(1);
    expect(requests).toEqual([]);
  });
});

linuxTest("image_gen rechecks the destination after credentials and before dispatch", async () => {
  await imageHost(async ({ outside, requests, draw, answer }) => {
    expect((await draw()).isError).toBeTrue();
    await answer(CHOICE.once);
    expect((await draw()).isError).toBeTrue();
    expect(requests).toEqual([]);
    expect(readdirSync(outside)).toEqual([]);
    expect(readdirSync(`${outside}-moved`)).toEqual([]);
  }, undefined, outside => {
    renameSync(outside, `${outside}-moved`);
    mkdirSync(outside);
  });
});

linuxTest("image_gen does not publish into a relocated approved parent or its protected replacement", async () => {
  await imageHost(async ({ host, outside, requests, draw, answer }) => {
    expect((await draw()).isError).toBeTrue();
    await answer(CHOICE.once);
    const relocated = await draw();
    expect(relocated.isError).toBeTrue();
    expect(relocated.text).toContain("destination changed");
    expect(requests).toHaveLength(1);
    expect(host.pendingOwnerQuestions()).toEqual([]);
    expect(readdirSync(`${outside}-moved`)).toEqual([]);
    expect(readdirSync(outside)).toEqual([]);
  }, outside => {
    const state = join(`${outside}-state`, ".corral-next");
    mkdirSync(state, { recursive: true });
    renameSync(outside, `${outside}-moved`);
    symlinkSync(state, outside);
  });
}, 30_000);
