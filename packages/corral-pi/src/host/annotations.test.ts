import { expect, test } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { Type } from "typebox";
import { ROLES, type RoleContext, type RoleModule } from "../roles/types.ts";
import { CorralCallError, type CorralCall, type CorralReceipt } from "../tools/corral.ts";
import { codemodeTool } from "../capabilities/codemode.ts";
import { CodeRuntime } from "../extensions/codemode/runtime.ts";
import { roleDir } from "./modules.ts";
import { AnnotationsViewSchema, newAnnotationRequestKey, readAnnotations, submitOwnerAnnotation,
  type OwnerAnnotationAction, type OwnerAnnotationClient } from "./annotations.ts";

// Captured from the Rust host tests via CORRAL_ANNOTATION_HOST_VIEWS; compact JSON keeps
// wire fixtures separate from hand-built TS objects. Set the same env to validate fresh output.
const RUST_VIEWS: unknown[] = JSON.parse(String.raw`[
{"candidate":{"candidate":{"base":"6118e62e49a609e9b9c490ff6f0bb968ef5f204b","head":"bcd209eca6769b67bae70458de48a52da1e6d58f","run":"R1","tree":"9dae7dc196fa859d82f17fcd0756173a8b238422"},"status":"available"},"counts":{"entries":4,"next_thread":2,"text_bytes":64,"threads":1},"entries":[{"at":"2026-11-08T23:47:48.932024357Z","author":{"kind":"owner","surface":{"container":{"kind":"worker","run":"R1","task":"T-001"},"generation":1,"kind":"pi_panel","session":"annotation-session"},"version":1},"kind":"opened","request":{"key":"one","payload_sha256":"4a608e943ff43aad7c651135b03d26fc81da3811b11fe52fe416370fe33b073f"},"seq":1,"state":"open","text":"Owner note","version":1}],"kind":"thread","next_cursor":"c6ab8d20aa6024c4cbf939f463f7593a55c5bdd9939d2b131d330574f7dda8df:7b2276657273696f6e223a312c227461736b223a22542d303031222c22746872656164223a224e31222c227374617465223a6e756c6c2c22616e63686f72223a6e756c6c2c22736e617073686f74223a5b5b224e31222c345d5d2c22706f73223a317d","paths":["code"],"paths_truncated":false,"task":"T-001","thread":{"anchor_status":{"status":"current"},"id":"N1","revision":4,"state":"open"},"upper_revision":4,"version":1},
{"candidate":{"candidate":{"base":"6118e62e49a609e9b9c490ff6f0bb968ef5f204b","head":"bcd209eca6769b67bae70458de48a52da1e6d58f","run":"R1","tree":"9dae7dc196fa859d82f17fcd0756173a8b238422"},"status":"available"},"counts":{"entries":4,"next_thread":2,"text_bytes":64,"threads":1},"items":[{"anchor":{"base":"6118e62e49a609e9b9c490ff6f0bb968ef5f204b","blob":"294186e497a23bf3fbfde12aacc7f720f668fe9a","excerpt":"after\n","excerpt_sha256":"7b9a72466d3960eb2aacccfc848939453490db0678bd4725def3f789b891c919","excerpt_truncated":false,"head":"bcd209eca6769b67bae70458de48a52da1e6d58f","path":"code","range":{"end":1,"start":1},"run":"R1","side":"new","task":"T-001","tree":"9dae7dc196fa859d82f17fcd0756173a8b238422","version":1},"anchor_status":{"status":"current"},"id":"N1","revision":4,"state":"open"}],"kind":"list","paths":["code"],"paths_truncated":false,"task":"T-001","version":1},
{"candidate":{"candidate":{"base":"48dae1dee78fb6389a13439ec193d8854e55fdd2","head":"63726ea582a37995b8f0b64faa9e4379a1588791","run":"R1","tree":"9dae7dc196fa859d82f17fcd0756173a8b238422"},"status":"available"},"counts":{"entries":4,"next_thread":2,"text_bytes":64,"threads":1},"entries":[{"at":"2026-11-08T23:47:49.314791145Z","author":{"kind":"owner","surface":{"container":{"kind":"reviewer","review":"R1-review1","run":"R1","task":"T-001"},"generation":1,"kind":"pi_panel","session":"annotation-session"},"version":1},"kind":"opened","request":{"key":"one","payload_sha256":"70f149606a4fc2684ae35b8bc20acb1ead374d9b4e3344fae719f73ce7ad1c51"},"seq":1,"state":"open","text":"Owner note","version":1}],"kind":"thread","next_cursor":"c6ab8d20aa6024c4cbf939f463f7593a55c5bdd9939d2b131d330574f7dda8df:7b2276657273696f6e223a312c227461736b223a22542d303031222c22746872656164223a224e31222c227374617465223a6e756c6c2c22616e63686f72223a6e756c6c2c22736e617073686f74223a5b5b224e31222c345d5d2c22706f73223a317d","paths":["code"],"paths_truncated":false,"task":"T-001","thread":{"anchor_status":{"status":"current"},"id":"N1","revision":4,"state":"open"},"upper_revision":4,"version":1},
{"candidate":{"candidate":{"base":"48dae1dee78fb6389a13439ec193d8854e55fdd2","head":"63726ea582a37995b8f0b64faa9e4379a1588791","run":"R1","tree":"9dae7dc196fa859d82f17fcd0756173a8b238422"},"status":"available"},"counts":{"entries":4,"next_thread":2,"text_bytes":64,"threads":1},"entries":[{"at":"2026-11-08T23:47:49.663940293Z","author":{"kind":"owner","surface":{"container":{"kind":"pm"},"generation":1,"kind":"pi_panel","session":"annotation-session"},"version":1},"kind":"opened","request":{"key":"one","payload_sha256":"70f149606a4fc2684ae35b8bc20acb1ead374d9b4e3344fae719f73ce7ad1c51"},"seq":1,"state":"open","text":"Owner note","version":1}],"kind":"thread","next_cursor":"c6ab8d20aa6024c4cbf939f463f7593a55c5bdd9939d2b131d330574f7dda8df:7b2276657273696f6e223a312c227461736b223a22542d303031222c22746872656164223a224e31222c227374617465223a6e756c6c2c22616e63686f72223a6e756c6c2c22736e617073686f74223a5b5b224e31222c345d5d2c22706f73223a317d","paths":["code"],"paths_truncated":false,"task":"T-001","thread":{"anchor_status":{"status":"current"},"id":"N1","revision":4,"state":"open"},"upper_revision":4,"version":1},
{"candidate":{"candidate":{"base":"48dae1dee78fb6389a13439ec193d8854e55fdd2","head":"63726ea582a37995b8f0b64faa9e4379a1588791","run":"R1","tree":"9dae7dc196fa859d82f17fcd0756173a8b238422"},"status":"available"},"counts":{"entries":4,"next_thread":2,"text_bytes":64,"threads":1},"entries":[{"at":"2026-11-08T23:47:49.943517717Z","author":{"kind":"owner","surface":{"container":{"kind":"assistant","session":"A-1"},"generation":1,"kind":"pi_panel","session":"annotation-session"},"version":1},"kind":"opened","request":{"key":"one","payload_sha256":"70f149606a4fc2684ae35b8bc20acb1ead374d9b4e3344fae719f73ce7ad1c51"},"seq":1,"state":"open","text":"Owner note","version":1}],"kind":"thread","next_cursor":"c6ab8d20aa6024c4cbf939f463f7593a55c5bdd9939d2b131d330574f7dda8df:7b2276657273696f6e223a312c227461736b223a22542d303031222c22746872656164223a224e31222c227374617465223a6e756c6c2c22616e63686f72223a6e756c6c2c22736e617073686f74223a5b5b224e31222c345d5d2c22706f73223a317d","paths":["code"],"paths_truncated":false,"task":"T-001","thread":{"anchor_status":{"status":"current"},"id":"N1","revision":4,"state":"open"},"upper_revision":4,"version":1},
{"candidate":{"reason":"worktree_missing","status":"unavailable"},"counts":{"entries":0,"next_thread":0,"text_bytes":0,"threads":0},"items":[],"kind":"list","paths":[],"paths_truncated":false,"task":"T-001","version":1}
]`);
const unavailable = { version: 1, task: "T-001", kind: "list", items: [],
  candidate: { status: "unavailable", reason: "worktree_missing" },
  counts: { threads: 0, entries: 0, text_bytes: 0, next_thread: 0 }, paths: [], paths_truncated: false };
const open: OwnerAnnotationAction = { action: "open", task: "T-001", expected_run: "R1",
  base: "b".repeat(40), head: "a".repeat(40), path: "code", side: "new", text: "Check the bound" };
const result = { task: "T-001", thread: "N1", entry: 1, revision: 1, state: "open", replayed: false };
const receipt = (result: unknown, state: CorralReceipt["state"] = "committed"): CorralReceipt =>
  ({ version: 1, request_id: "ev-1-1", op: "event", state, result });
function client(answer: () => CorralReceipt | Promise<CorralReceipt>): OwnerAnnotationClient & { calls: CorralCall[] } {
  let seq = 0;
  const calls: CorralCall[] = [];
  return { calls, nextEventRequestId: () => `ev-1-${++seq}`,
    call: async (_op, call) => { calls.push(call); return answer(); } };
}

test("all Rust view fixtures parse and unavailable is explicit, not an empty successful list", async () => {
  const views = [...RUST_VIEWS, unavailable];
  if (process.env.CORRAL_ANNOTATION_HOST_VIEWS) {
    for (const file of readdirSync(process.env.CORRAL_ANNOTATION_HOST_VIEWS).filter(f => f.endsWith(".json"))) {
      const value: unknown = JSON.parse(readFileSync(join(process.env.CORRAL_ANNOTATION_HOST_VIEWS, file), "utf8"));
      views.push(...(Array.isArray(value) ? value : [value]));
    }
  }
  for (const view of views) expect(AnnotationsViewSchema.safeParse(view).success).toBeTrue();
  const read = await readAnnotations(client(() => receipt(unavailable)), { task: "T-001" });
  expect(read.state).toBe("unavailable");
  if (read.state === "unavailable") expect(read.view.candidate).toEqual(unavailable.candidate);
  expect(AnnotationsViewSchema.safeParse({ ...unavailable, version: 2 }).success).toBeFalse();
  expect(AnnotationsViewSchema.safeParse({ ...unavailable, paths: ["x".repeat(32768)] }).success).toBeFalse();
});

test("owner action preserves caller identity and one request key across retries and generations", async () => {
  let response = receipt(result);
  const host = client(() => response);
  const key = newAnnotationRequestKey(); expect(key).toMatch(/^an-[0-9a-f]{32}$/u);
  expect((await submitOwnerAnnotation(host, open, key)).state).toBe("committed");
  response = receipt({ ...result, replayed: true });
  const next = { ...host, nextEventRequestId: () => "ev-2-1" };
  expect((await submitOwnerAnnotation(next, open, key)).state).toBe("replayed");
  expect(host.calls.map(c => c.requestId)).toEqual(["ev-1-1", "ev-2-1"]);
  expect(host.calls[0]?.payload).toEqual(host.calls[1]?.payload);
  expect(host.calls[0]?.payload).toEqual({ kind: "owner_annotation", task: "T-001", request_key: key,
    action: { action: "open", expected_run: "R1", base: open.base, head: open.head, path: "code", side: "new", text: open.text } });
  response = receipt({ code: "annotation_refused", detail: { code: "thread_revision_conflict", message: "Conflict",
    detail: { revision: 2, state: "resolved" } } }, "rejected");
  expect(await submitOwnerAnnotation(host, { action: "reply", task: "T-001", thread: "N1", expected_revision: 1, text: "Reply" }, key))
    .toEqual({ state: "refused", code: "thread_revision_conflict", detail: { revision: 2, state: "resolved" } });
});

test("timeouts, transport/output errors, nonterminal receipts and malformed results never claim committed", async () => {
  for (const code of ["call_timeout", "call_failed", "call_output"]) {
    const host = client(() => { throw new CorralCallError(code, "failure"); });
    expect(await submitOwnerAnnotation(host, open, "an-stable")).toEqual({ state: "unknown", reason: code });
    expect((await readAnnotations(host, { task: "T-001" })).state).toBe("unknown");
    expect(host.calls).toHaveLength(2); // No automatic retry.
  }
  for (const state of ["reserved", "uncertain", "queued", "abandoned"] as const) {
    expect((await submitOwnerAnnotation(client(() => receipt(result, state)), open, "an-stable")).state).toBe("unknown");
  }
  expect((await submitOwnerAnnotation(client(() => receipt({})), open, "an-stable")).state).toBe("unknown");
  const strict = client(() => receipt(result));
  expect((await submitOwnerAnnotation(strict, { ...open, author: "owner" } as OwnerAnnotationAction, "an-stable")).state).toBe("unknown");
  expect(strict.calls).toHaveLength(0);
});

test("every role's direct and Code Mode tool table excludes owner_annotation and raw events", async () => {
  const runtime = new CodeRuntime({ generation: 1, emit() {}, journal() {}, notify: async () => "delivered" });
  for (const role of ROLES) {
    const module = await import(`../roles/${roleDir(role)}/index.ts`) as { default: RoleModule };
    const context: RoleContext = { identity: { role, store: "st-test", project: "corral", generation: 1,
      task: "T-001", run: "R1", assistant: "A-1" }, cwd: "/tmp", model: "fixture", todo: true,
      capabilities: { catalog: 8, digest: "d".repeat(64), preset: "standard", enabled: ["codemode", "todo", "ask_user"] } };
    const specs = module.default.tools(context);
    expect(specs.some(s => s.op === "event" || s.name === "owner_annotation" || s.name === "corral_owner_annotation")).toBeFalse();
    const table = [...module.default.upstreamTools, ...module.default.localToolNames ?? [], ...specs.map(s => s.name)].map(name => ({ name,
      label: name, description: name, parameters: Type.Object({}), execute: async () => ({ content: [], details: {} }) }));
    const tool = codemodeTool(runtime, names => names.filter(name => table.some(t => t.name === name)));
    const loadout = tool.prepareLoadout?.({ callable: table, declared: table, registered: table,
      getExposure: () => "direct", getNamespace: () => undefined, getPromptGuidelines: () => [] });
    expect(JSON.stringify(loadout)).not.toContain("owner_annotation");
    expect(JSON.stringify(loadout)).not.toContain("tools.event");
  }
});
