// Actual keyboard/mouse terminal captures, using herdr's output subscriptions.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";

export class PlanBoardCapture {
  constructor({ binary, root, evidence, columns, run, decisions = 0 }) {
    Object.assign(this, { binary, root, evidence, columns, run, decisions });
    this.serial = 0;
    this.onOutput = () => {};
    this.result = { status: "FAIL", columns, scenes: [] };
  }

  async herdr(...args) {
    return (await this.run("herdr", ["--session", "corral-test", ...args])).stdout;
  }

  rpc(method, params) {
    const id = `plan-board-${++this.serial}`;
    return new Promise((accept, reject) => {
      const connection = createConnection(this.socketPath);
      const streaming = method === "events.subscribe";
      if (streaming) { this.subscription?.destroy(); this.subscription = connection; }
      const timer = setTimeout(() => { connection.destroy(); reject(new Error(`${method}: timeout`)); }, 20000);
      connection.setEncoding("utf8");
      let buffer = "";
      connection.on("data", chunk => {
        buffer += chunk;
        let end;
        while ((end = buffer.indexOf("\n")) >= 0) {
          const message = JSON.parse(buffer.slice(0, end));
          buffer = buffer.slice(end + 1);
          if (message.id === id) {
            clearTimeout(timer);
            if (!streaming) connection.destroy();
            if (message.error) reject(new Error(JSON.stringify(message.error)));
            else accept(message.result);
          } else if (message.event === "pane.output_matched" && message.data.pane_id === this.pane) {
            this.lastOutput = message.data.read;
            this.onOutput(message.data.read);
          }
        }
      });
      connection.on("error", error => { clearTimeout(timer); reject(error); });
      connection.once("connect", () => connection.write(JSON.stringify({ id, method, params }) + "\n"));
    });
  }

  raw(text) { return this.rpc("pane.send_input", { pane_id: this.pane, text }); }

  async stage(name, marker, check, action, snapshot = true) {
    let timer;
    const matched = new Promise((accept, reject) => {
      timer = setTimeout(() => reject(new Error(`${name}: rendered state not observed`)), 20000);
      this.onOutput = read => {
        if (check(read.text)) {
          this.onOutput = () => {};
          if (snapshot) this.capture(name, read).then(accept, reject);
          else accept(read.text);
        }
      };
    });
    matched.catch(() => {});
    try {
      await this.rpc("events.subscribe", { subscriptions: [{
        type: "pane.output_matched", pane_id: this.pane, source: "visible",
        match: { type: "regex", value: marker }, strip_ansi: true,
      }] });
      await action();
      this.screen = await matched;
      return this.screen;
    } finally { clearTimeout(timer); this.onOutput = () => {}; }
  }

  async capture(name, read) {
    await writeFile(join(this.evidence, `${name}.txt`), read.text);
    const styled = (await this.rpc("pane.read", {
      pane_id: this.pane, source: "visible", format: "ansi", strip_ansi: false,
    })).read.text;
    assert.match(styled, /\x1b\[[0-9;]*(?:38|48);[25];/);
    await writeFile(join(this.evidence, `${name}.ansi`), styled);
    this.result.scenes.push(name);
    return read.text;
  }

  async click(needle) {
    const lines = this.screen.split("\n");
    const y = lines.findIndex(line => line.includes(needle));
    assert(y >= 0, `Missing target ${needle}\n${this.screen}`);
    const x = Bun.stringWidth(lines[y].slice(0, lines[y].indexOf(needle))) + 1;
    await this.raw(`\x1b[<0;${x};${y + 1}M\x1b[<0;${x};${y + 1}m`);
  }

  async start({ parent, tour = true, boardReady = s => s.includes("planning") && s.includes("q leave") } = {}) {
    await mkdir(this.evidence, { recursive: true });
    this.socketPath = (await this.herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
    assert(this.socketPath?.includes("/sessions/corral-test/"), "Refuse a non-isolated server");
    const created = JSON.parse(await this.herdr("workspace", "create", "--cwd", this.root,
      "--label", "T-100 plan board QA", "--no-focus")).result;
    this.workspace = created.workspace.workspace_id;
    this.pane = created.root_pane.pane_id;
    // stty alone does not resize herdr's terminal emulator. Control the owned
    // terminal so the PTY, capture and application all have the exact same size.
    this.viewport = Bun.spawn(["herdr", "--session", "corral-test", "terminal", "session",
      "control", this.pane, "--cols", String(this.columns), "--rows", "30"],
    { stdin: "pipe", stdout: "pipe", stderr: "pipe" });
    this.viewportErrors = new Response(this.viewport.stderr).text();
    const reader = this.viewport.stdout.getReader();
    let timer;
    try {
      const first = await Promise.race([
        reader.read(),
        new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("Viewport control not ready")), 20000); }),
      ]);
      if (first.done) throw new Error(`Viewport control closed: ${await this.viewportErrors}`);
      await writeFile(join(this.evidence, "viewport.json"), new TextDecoder().decode(first.value));
    } finally { clearTimeout(timer); }
    this.viewportOutput = (async () => { while (!(await reader.read()).done) {} })();
    const launch = () =>
      this.herdr("pane", "run", this.pane,
        `stty -F /dev/tty cols ${this.columns} rows 30; env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor ${quote(this.binary)} --root ${quote(this.root)} board </dev/null`);
    if (parent) {
      await this.stage("01-compact-parent", "decomposed into 2 tasks",
        s => s.split("\n").some(line => line.includes(parent) && line.includes("decomposed into 2 tasks")), launch);
      return;
    }
    if (!tour) {
      await this.stage("02-planning-board", "q leave", boardReady, launch);
      return;
    }
    await this.stage("01-tour", "skip tour", s => s.includes("skip tour"), launch);
    await this.stage("02-planning-board", "q leave", boardReady,
      () => this.raw("\x1b"));
  }

  async inspectAndApprove(id) {
    await this.stage("05-plan-detail", "Revisions", s => s.includes("Revisions") && s.includes("Your approval"),
      async () => { await this.click(id); await this.click(id); });
    await this.stage("06-plan-snapshot", "Risks", s => s.includes("Risks"),
      () => this.raw("\x1b[6~".repeat(12)));
    await this.stage("07-plan-top", "Revisions", s => s.includes("Revisions"),
      () => this.raw("\x1b[H"));
    await this.stage("07a-owner-changes", "request plan changes", s => s.includes("request plan changes P1"),
      () => this.click("Request changes"));
    await this.raw("Keep these owner notes");
    await this.stage("07c-discard-confirmation", "Discard owner notes", s => s.includes("Discard owner notes?"),
      () => this.click("Cancel"));
    await this.stage("07d-notes-preserved", "request plan changes P1 of", s => s.includes("Keep these owner notes"),
      () => this.click("Keep editing"));
    await this.stage("07e-discard-again", "Discard owner notes", s => s.includes("Discard owner notes?"),
      () => this.raw("\x1b"));
    await this.stage("07b-changes-cancelled", "pgup/pgdn", s => s.includes("Revisions") && !s.includes("request plan changes P1"),
      () => this.raw("\r"));
    await this.stage("08-owner-approve", "approve plan P1 of", s => s.includes("approve plan P1"),
      () => this.raw("a"));
    if (this.decisions) await this.resolveTallForm();
    await this.stage("09-decomposed", "decomposed", s => s.includes("decomposed into 2") && s.includes("widget-api"),
      () => this.decisions ? this.click("[Approve]") : this.raw("\r"));
  }

  async resolveTallForm() {
    assert(this.screen.includes("[Approve]") && this.screen.includes("[Cancel]"));
    for (let i = 0; i <= this.decisions; i++) {
      const answer = `keyboard${i}`;
      await this.stage(`08-keyboard-${i}`, answer, s => s.includes(answer),
        () => this.raw(`${i ? "\t" : ""}${answer}`),
        i === 5 || i === this.decisions - 1 || i === this.decisions);
    }
    const reverse = `resolution › keyboard${this.decisions - 1}▏`;
    await this.stage("08-keyboard-reverse", reverse,
      s => s.includes(reverse), () => this.raw("\x1b[Z"));
    await this.stage("08-discard-keyboard", "Discard owner notes",
      s => s.includes("Discard owner notes?"), () => this.click("[Cancel]"), false);
    await this.stage("08-reopen-ready", "\\[Approve plan\\]", s => s.includes("[Approve plan]"),
      () => this.click("[Discard]"), false);
    await this.stage("08-mouse-form", "approve plan P1 of",
      s => s.includes("resolution") && s.includes("[Cancel]"), () => this.click("[Approve plan]"));
    for (let i = 1; i <= this.decisions; i++) {
      const fieldRow = screen => {
        const lines = screen.split("\n");
        const row = lines.findIndex(line => new RegExp(`decision: Owner decision ${i}(?:\\s|│)`).test(line));
        return row >= 0 && lines[row + 1]?.includes("resolution ›") ? row + 1 : -1;
      };
      if (fieldRow(this.screen) < 0) {
        await this.stage(`08-wheel-${i}`, "note ›", s => fieldRow(s) >= 0 && s.includes("note ›"),
          () => this.raw("\x1b[<65;40;15M".repeat(2)));
      }
      const y = fieldRow(this.screen) + 1;
      const x = this.screen.split("\n")[y - 1].indexOf("resolution") + 1;
      const answer = `resolve${i}`;
      await this.stage(`08-mouse-${i}`, answer, s => new RegExp(`resolution › ${answer}(?:\\s|▏)`).test(s),
        async () => {
          await this.raw(`\x1b[<0;${x};${y}M\x1b[<0;${x};${y}m`);
          await this.raw(answer);
        }, i === 1 || i === 6 || i === this.decisions);
    }
    assert(this.screen.includes("note ›"), "The note must also be reachable");
    assert(this.screen.includes("[Approve]") && this.screen.includes("[Cancel]"));
  }

  async openChild(id) {
    await this.stage("10-child-link", "Governed by", s => s.includes(id) && s.includes("widget-api") && !s.includes("Revisions"),
      () => this.click(`▸ ${id}`));
  }

  async close(pass) {
    this.result.status = pass ? "PASS" : "FAIL";
    this.result.cleanup = "subscription destroyed; owned viewport controller terminated; owned board workspace closed";
    this.subscription?.destroy();
    if (!pass && this.pane) {
      await writeFile(join(this.evidence, "99-last-event.json"), JSON.stringify(this.lastOutput ?? null, null, 2));
      const read = (await this.rpc("pane.read", { pane_id: this.pane, source: "visible", strip_ansi: true })).read;
      await this.capture("99-failure", read);
    }
    if (this.viewport) {
      this.viewport.kill();
      await this.viewport.exited;
      await this.viewportOutput;
      await this.viewportErrors;
    }
    if (this.workspace) await this.herdr("workspace", "close", this.workspace);
    await writeFile(join(this.evidence, "result.json"), JSON.stringify(this.result, null, 2) + "\n");
  }
}
