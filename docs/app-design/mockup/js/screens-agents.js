// Agents wall, agent session (transcript ⇄ terminal), Usage.
(() => {
  const { esc, ic, mk, rail, T, illus, money } = U;
  const stateChip = (a) => ({ live: "corral", working: "worker", asking: "you", gating: "corral", idle: "" }[a.state] ?? "");
  const tailOf = (a) => a.tail || T(a.task)?.tail || ["…"];
  const termLines = (lines) => lines.map((l) => (/PASS|ok |pass/.test(l) ? `<span class="ok">${esc(l)}</span>` : /^(\$|Edit|Bash|Read|wake|read|Ask)/.test(l) ? `<span class="p">${esc(l)}</span>` : esc(l))).join("\n");

  // ---------- Agents ----------
  SCREENS.agents = {
    desk(S) {
      const tiles = D.AGENTS.map((a) => `<button class="tile" data-act="open-session" data-id="${a.task || "PM"}">
        <div class="th"><span class="chip ${stateChip(a)}">${a.state === "asking" ? "needs you" : a.state}</span><span class="id">${a.id}</span><span class="dim" style="margin-left:auto">${a.role}</span></div>
        <div class="tt2">${esc(a.title)}</div>
        <div class="doing ${a.state === "asking" ? "c-you" : ""}">${esc(a.doing)}</div>
        <div class="term" style="position:relative">${/legacy/.test(a.runtime) ? `<span class="tlab snap">snapshot · 20s ago</span>` : `<span class="tlab live">live</span>`}${termLines(tailOf(a).slice(-4))}</div>
        <div class="tf"><span>${esc(a.profile)}</span><span>${esc(a.runtime)}</span><span class="mono">${a.pane}</span><span class="r">since ${a.since}</span></div></button>`).join("");
      return `${connBanner()}<div class="pagehead"><h1>Agents</h1><span class="sub">the quick thread first, then the PM, workers, gates and assistant sessions · what each is doing now, and where</span><span class="acts"><div class="seg"><button aria-pressed="true">Wall</button><button>List</button></div></span></div>
        <div class="pane stalefx" data-scroll="agents" style="flex:1"><div class="machine">${ic("term")}<b>workstation</b><span class="dim">daemon host · herdr-core 0.1 · 6 agents · Herdr TUI (legacy) · 1 agent</span>${illus("machine name illustrative")}<span class="dim" style="margin-left:auto">Other machines join over SSH later (Settings › Machines)</span></div>
        <div class="wall">${ASKUI.qtTile()}${tiles}</div>
        <div class="cols" style="grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:24px;padding:4px 18px 18px;align-items:start"><div>${ASKUI.sessionsBlock(false)}</div><div>${ASKUI.filedBlock(false)}</div></div>
        <div class="illus" style="padding:0 18px 16px">Tails are illustrative; agents, panes, profiles and states follow the 22:12 snapshot.</div></div>`;
    },
    phone(S) {
      const rows = D.AGENTS.map((a) => `<button class="prow" data-act="open-session" data-id="${a.task || "PM"}"><span class="dot" style="color:var(--${a.state === "asking" ? "you" : a.state === "working" ? "worker" : a.state === "idle" ? "ink-4" : "corral"});grid-row:1"></span><span class="l1"><span class="id">${a.id}</span><span class="tt">${esc(a.title)}</span></span><span class="mono dim" style="font-size:12px">${a.since}</span><span class="l2"><span class="nw ${a.state === "asking" ? "c-you" : ""}">${esc(a.doing)}</span><span class="r" style="font-family:var(--font)">${/legacy/.test(a.runtime) ? "snapshot" : "live"}</span></span></button>`).join("");
      return { title: "Agents", large: true, body: `${ASKUI.qtRow()}<div class="ph-sec">workstation<span class="n">7</span><span class="r">daemon host ${illus("name illustrative")}</span></div>${rows}${ASKUI.sessionsBlock(true)}${ASKUI.filedBlock(true)}` };
    },
  };

  // ---------- Session ----------
  const tool = (kind, arg, res, open, out) => `<div class="tool"><button class="tl" data-act="toast" data-msg="Tool call expanded">${mk(res === "fail" ? "fail" : "done")}<span class="tk">${kind}</span><span class="ta">${esc(arg)}</span><span class="tr">${esc(res === "fail" ? "failed" : res)}</span></button>${open ? `<div class="term">${termLines(out)}</div>` : ""}</div>`;
  function transcript(S, id, phone) {
    if (id !== "T-868") {
      const a = D.AGENTS.find((x) => x.task === id) || D.AGENTS[0];
      return `<div class="tx"><div class="tm"><span class="av w">W</span><div><div class="hd"><b>worker ${a.id}</b> · ${a.since}</div><p>${esc(a.doing)}</p>${tailOf(a).filter((l) => /^(Edit|Bash|Read)/.test(l)).map((l) => tool(l.split(/\s+/)[0], l.replace(/^\S+\s+/, ""), "ok")).join("")}<div class="illus">transcript illustrative</div></div></div></div>`;
    }
    return `<div class="tx">
      <div class="tm"><span class="av p">C</span><div><div class="hd"><b>Corral</b> · 18:58</div><p class="dim">Run R1 admitted on pi-sol xhigh · governed by <span class="mono">T-701@P2</span> · write scope 20 paths · herdr-core pane <span class="mono">w12C:p2</span></p></div></div>
      <div class="tm"><span class="av w">W</span><div><div class="hd"><b>worker T-868</b> · 18:59</div><p>Verifying installed Pi package metadata is 1.5.0 and refreshing its frozen dependencies in this worktree.</p>
        ${tool("Bash", "bun install --frozen-lockfile", "ok 6.2s")}${tool("Read", "packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.5.0.patch", "612 lines")}</div></div>
      <div class="tm"><span class="av w">W</span><div><div class="hd"><b>worker T-868</b> · 19:08</div><p>Stable identities are in: <span class="mono-inline">q-g&lt;generation&gt;-&lt;bind&gt;-&lt;counter&gt;</span>. Lifecycle entries persist before any queue mutation.</p>
        ${tool("Edit", "packages/corral-pi/src/host/queue.ts", "+41 −6")}${tool("Bash", "bun test test/queue-host.test.ts", "16 pass", true, ["$ bun test test/queue-host.test.ts", " ✓ withdraw targets one identity", " ✓ start race: started persists before drain", " ✓ replay returns stored outcome", " 16 pass · 0 fail · 1.64s"])}</div></div>
      <div class="tm"><span class="av w">W</span><div><div class="hd"><b>worker T-868</b> asked you · 19:12 · escalated by the PM 19:14</div>
        <div class="qcard"><div class="qk"><span class="dot"></span>T-868/R1/Q1 · owner only · blocking</div><p>${esc(D.Q.short)}</p>
          <div style="display:flex;gap:8px;flex-wrap:wrap"><button class="btn" style="height:28px" data-act="goto" data-to="question">Open handover · decide</button><span class="dim" style="font-size:12px;align-self:center">2 options · worker recommends 1</span></div></div>
        <p>I can implement the ledger and other controls independently while awaiting this decision.</p></div></div>
      <div class="tm"><span class="av w">W</span><div><div class="hd"><b>worker T-868</b> · 19:40</div>${tool("Edit", "packages/corral-pi/src/bridge/queue.ts", "+88 −3")}${tool("Bash", "bun run --bun typecheck", "ok 11.0s")}</div></div>
      <div class="tm"><span class="av o">Y</span><div><div class="hd"><b>you</b> · 21:58 · <span class="mono">via=app:owner-iphone</span></div><p>Does option 1 change anything in rich mode?</p></div></div>
      <div class="tm"><span class="av w">W</span><div><div class="hd"><b>worker T-868</b> · 21:59</div><p>No. Rich mode stays read-only/count-only; neither option restores there.</p><p class="dim" style="font-size:12px">idle since 21:40 · waiting on T-868/R1/Q1</p></div></div>
      <div class="illus" style="padding:4px 24px 16px">Transcript illustrative; the question, its options and the tool targets are real.</div></div>`;
  }
  function bigTerm(id, phone) {
    const lines = id === "T-868" ? `<span class="w8">pi 1.5.0 · corral bound session · T-868 R1 · pi-sol xhigh</span>
<span class="w8">───────────────────────────────────────────────────────────</span>
<span class="b">●</span> Edit packages/corral-pi/src/bridge/queue.ts
  <span class="ok">+88</span> <span class="er" style="color:oklch(0.72 0.17 25)">-3</span>
<span class="b">●</span> Bash bun run --bun typecheck
  <span class="ok">ok</span> 11.0s

<span class="y">? T-868/R1/Q1 · waiting on the owner</span>
  Restore policy: pane-origin input only, or all origins?
  1. Restore only pane-origin input; leave Corral/extension items queued
  2. Retain restore-all; journal removals, resubmit as new pane input

<span class="w8">owner (app:owner-iphone):</span> Does option 1 change anything in rich mode?
<span class="b">●</span> No. Rich mode stays read-only/count-only; neither option restores there.

<span class="w8">queued  0 steering · 0 follow-ups · queued, not execution</span>
<span class="p">›</span> <span class="cur"></span>` : termLines(tailOf(D.AGENTS.find((a) => a.task === id) || D.AGENTS[0])) + `\n<span class="cur"></span>`;
    return lines;
  }
  function sessHeader(S, id, phone) {
    const t = T(id);
    const seg = `<div class="${phone ? "pseg" : "seg"}" role="tablist" aria-label="Session view"><button data-act="set" data-k="sessionView" data-v="transcript" aria-pressed="${S.sessionView === "transcript"}">${phone ? "" : ic("chat")} Transcript</button><button data-act="set" data-k="sessionView" data-v="terminal" aria-pressed="${S.sessionView === "terminal"}">${phone ? "" : ic("term")} Terminal</button></div>`;
    return seg;
  }
  SCREENS.session = {
    desk(S) {
      const id = S.session === "PM" ? "T-868" : S.session, t = T(id);
      const view = S.sessionView === "terminal"
        ? (() => { const legacy = id === "T-854"; return `<div style="display:flex;flex-direction:column;flex:1;min-height:0"><div class="termbar"><span>${legacy ? "Herdr TUI (legacy)" : "herdr-core"} · ${t.pane || "w12C:p2"} · 140×46</span><span>${legacy ? "snapshot · refreshed 20s ago · no live stream on the legacy runtime" : "live stream · scrollback 1,000"}</span><span style="margin-left:auto">${legacy ? "read-only snapshot" : "keys go to the agent's pane as owner input · actor=owner"}</span></div><div class="bigterm" data-scroll="term">${bigTerm(id)}</div></div>`; })()
        : `<div class="cols" style="grid-template-columns:minmax(0,1fr) 280px;flex:1;min-height:0"><div style="display:flex;flex-direction:column;min-height:0"><div class="scroll" style="flex:1" data-scroll="tx">${transcript(S, id)}</div>
            <div class="composer"><div class="box"><textarea placeholder="Message worker ${id}…" aria-label="Reply to agent"></textarea><button class="btn" data-act="toast" data-msg="Sent to ${id} via corral send · actor=owner via=app:owner-desktop">${ic("send")}Send</button></div><div class="hint"><span>Sends with <span class="mono">corral send ${id}</span> · arrives as owner input</span><span>Answering the open question happens on its handover sheet</span></div></div></div>
            <div class="pane side-col" style="padding:14px 16px"><div class="h3" style="margin-top:0">Run</div><dl class="kv"><dt>Task</dt><dd><span class="mono">${id}</span> ${esc(t.title)}</dd><dt>Rail</dt><dd>${rail(t.rail)}</dd><dt>Agent</dt><dd>${t.agent || "pi-sol xhigh"}</dd><dt>Runtime</dt><dd>herdr-core · <span class="mono">${t.pane || "—"}</span></dd><dt>Machine</dt><dd>workstation</dd><dt>Started</dt><dd>${t.started || "—"}</dd></dl>
            <div class="h3">Steps</div><ul class="steps">${(t.steps || []).map(([k, s]) => `<li class="${k}">${mk(k, true)}<span>${esc(s)}</span></li>`).join("")}</ul></div></div>`;
      return `${connBanner()}<div class="pagehead"><button class="btn quiet" data-act="goto" data-to="agents">${ic("back")}Agents</button><h1><span class="mono dim" style="font-weight:500">${id}</span> ${esc(t.title)}</h1><span class="sub">worker R1 · ${t.agent || ""}</span><span class="acts"><span class="dim" style="font-size:12px"><span class="kbd">\`</span> switch</span>${sessHeader(S, id)}</span></div>
        <div class="stalefx" style="flex:1;min-height:0;display:flex;flex-direction:column">${view}</div>`;
    },
    phone(S) {
      const id = S.session === "PM" ? "T-868" : S.session;
      const term = S.sessionView === "terminal";
      const body = sessHeader(S, id, true) + (term
        ? `<div class="bigterm" style="font-size:10.5px;padding:12px;min-height:520px;white-space:pre-wrap">${bigTerm(id, true)}</div>`
        : transcript(S, id, true).replace(/class="tm"/g, 'class="tm" style="padding:8px 16px"'));
      const decide = term
        ? `<div style="display:flex;gap:6px;width:100%;font-family:var(--mono);font-size:13px">${["Esc", "Tab", "Ctrl", "↑", "↓", "⏎"].map((k) => `<button class="pbtn ghost" style="flex:1;padding:0;font-size:13px;min-height:40px">${k}</button>`).join("")}</div>`
        : `<div class="reply" style="margin:0;flex:1"><textarea placeholder="Message worker ${id}…" style="font-size:15px"></textarea><button class="pbtn" style="min-height:36px;padding:0 12px" data-act="toast" data-msg="Sent · via=app:owner-iphone">${ic("send")}</button></div>`;
      return { title: `${id} session`, backLabel: "Back", body, decide, right: `<button data-act="set" data-k="sessionView" data-v="${term ? "transcript" : "terminal"}" aria-label="Switch view">${ic(term ? "chat" : "term")}</button>` };
    },
  };

  // ---------- Usage ----------
  function bandChart(p, w, h) {
    const pad = 22, ph = h - 18, X = (i) => pad + (i / (p.series.length - 1)) * (w - pad - 4), Y = (v) => 4 + (1 - v) * (ph - 8);
    const z = `<rect class="z2" x="${pad}" y="${Y(1)}" width="${w - pad - 4}" height="${Y(0.9) - Y(1)}"/><rect class="z1" x="${pad}" y="${Y(0.9)}" width="${w - pad - 4}" height="${Y(0.7) - Y(0.9)}"/>
      <text class="zl" x="0" y="${Y(0.95) + 3}">90</text><text class="zl" x="0" y="${Y(0.8) + 3}">70</text><text class="zl" x="0" y="${Y(0) }">0</text>`;
    const pts = p.series.map((v, i) => `${X(i)},${Y(v)}`).join(" ");
    const last = p.series.length - 1;
    const ax = D.USAGE.hours.map((hh, i) => (i % 3 === 0 ? `<text class="ax" x="${X(i)}" y="${h - 2}" text-anchor="middle">${hh}</text>` : "")).join("");
    return `<svg class="band-chart" viewBox="0 0 ${w} ${h}" role="img" aria-label="${p.label} window use today, ${Math.round(p.used * 100)} percent now">${z}<polyline class="ln" points="${pts}"/><circle class="pt" cx="${X(last)}" cy="${Y(p.series[last])}" r="3"/>${ax}</svg>`;
  }
  const lvl = (u) => (u >= 0.9 ? "bad" : u >= 0.7 ? "warn" : "");
  SCREENS.usage = {
    desk(S) {
      const cells = D.USAGE.providers.map((p) => `<div class="ucell"><div class="uh"><b>${esc(p.label)}</b><span class="dim" style="font-size:11.5px">${esc(p.window)}</span></div>
        <div style="display:flex;align-items:baseline;gap:14px;margin-top:6px"><span class="big">${Math.round(p.used * 100)}%<small>used</small></span><span class="${lvl(p.used) ? "c-held" : "dim"}" style="font-weight:600">${Math.round((1 - p.used) * 100)}% headroom</span><span class="dim" style="font-size:12px;margin-left:auto">resets ${p.resets.includes(":") ? "at " : "in "}${p.resets}</span></div>
        ${bandChart(p, 380, 96)}<div class="dim" style="font-size:12px;margin-top:4px">today ${money(p.today)} · 7 days ${money(p.week)}${p.weekly != null ? ` · weekly cap ${Math.round(p.weekly * 100)}%` : ""}</div></div>`).join("");
      const rows = D.USAGE.providers.flatMap((p) => p.profiles.map(([name, u, c], i) => `<tr><td>${i === 0 ? esc(p.label) : ""}</td><td class="mono">${esc(name)}</td><td class="n">${Math.round(u * 100)}%</td><td><span class="meter"><i class="${lvl(p.used)}" style="width:${Math.round(u * 100)}%"></i></span></td><td class="n">${money(c)}</td><td class="n">${i === 0 ? Math.round((1 - p.used) * 100) + "%" : ""}</td></tr>`)).join("");
      const maxW = 100;
      const bars = D.USAGE.week.map(([d, a, b, c]) => `<div style="display:grid;grid-template-columns:34px 1fr 52px;gap:8px;align-items:center;font-size:12px"><span class="dim">${d}</span><span style="display:flex;height:10px;border-radius:2px;overflow:hidden"><i style="width:${a}%;background:var(--ink-2)"></i><i style="width:${b}%;background:var(--ink-4)"></i><i style="width:${c}%;background:var(--held)"></i></span><span class="mono dim" style="text-align:right">$${a + b + c}</span></div>`).join("");
      return `${connBanner()}<div class="pagehead"><h1>Usage</h1><span class="sub">cost and quota per provider and profile · visibility only, no routing</span><span class="acts"><div class="seg"><button aria-pressed="true">Today</button><button>7 days</button><button>30 days</button></div></span></div>
        <div class="pane stalefx" data-scroll="usage" style="flex:1"><div class="ugrid">${cells}</div>
        <div class="cols" style="grid-template-columns:minmax(0,1fr) 380px"><div style="padding:6px 18px 18px"><div class="h3">By profile · current window</div><table class="utable"><thead><tr><th>Provider</th><th>Profile</th><th class="n">Window</th><th>Share · 80% marker</th><th class="n">Today</th><th class="n">Headroom</th></tr></thead><tbody>${rows}</tbody></table></div>
        <div style="padding:6px 18px 18px;border-left:1px solid var(--rule)"><div class="h3">Cost per day · 7 days</div><div style="display:grid;gap:7px">${bars}</div><div class="legend" style="padding:10px 0 0"><span><i style="background:var(--ink-2)"></i>OpenAI sub</span><span><i style="background:var(--ink-4)"></i>Anthropic sub</span><span><i style="background:var(--held)"></i>metered API</span></div></div></div>
        <div class="illus" style="padding:0 18px 16px">All numbers illustrative. Connection names follow real usage records (openai-subscription); bands mark 70% and 90% of the provider window.</div></div>`;
    },
    phone(S) {
      const cards = D.USAGE.providers.map((p) => `<div class="pband"><div class="bh" style="text-transform:none;letter-spacing:0;font-size:15px;color:var(--ink)">${esc(p.label)}<span class="r">${esc(p.window)}</span></div>
        <div style="display:flex;align-items:baseline;gap:12px"><span class="big" style="font-size:30px;font-weight:700">${Math.round(p.used * 100)}%</span><span class="${lvl(p.used) ? "c-held" : "dim"}" style="font-weight:700">${Math.round((1 - p.used) * 100)}% headroom</span><span class="dim" style="margin-left:auto;font-size:13px">resets ${p.resets}</span></div>
        ${bandChart(p, 354, 92)}<div style="display:grid;gap:4px;margin-top:8px;font-size:14px">${p.profiles.map(([n, u, c]) => `<div style="display:flex;gap:8px"><span class="mono">${esc(n)}</span><span class="dim" style="margin-left:auto">${Math.round(u * 100)}% · ${money(c)}</span></div>`).join("")}</div></div>`).join("");
      return { title: "Usage", backLabel: "More", body: `<div class="pseg"><button aria-pressed="true">Today</button><button>7 days</button><button>30 days</button></div>${cards}<div class="illus" style="padding:10px 18px 24px">All numbers illustrative.</div>` };
    },
  };
})();
