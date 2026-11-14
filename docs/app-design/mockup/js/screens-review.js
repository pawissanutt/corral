// Review: candidate-bound verdicts, split/unified diff with inline comments, rendered evidence.
(() => {
  const { esc, ic, mk, illus } = U;
  const FILES = [
    { path: "packages/corral-pi/test/cross-lane.test.ts", add: 1, del: 1, illus: true, hunk: "@@ -212,9 +212,9 @@ describe(\"cross-lane paging\")",
      lines: [[" ", 212, 212, "  const lane = await privateHerdr({ plugins: false });"], ["-", 213, null, "  const inputs = 256;", "256"], ["+", null, 213, "  const inputs = 128;", "128"], [" ", 214, 214, "  for (let i = 0; i < inputs; i++) await submit(lane, `msg ${i}`);"], [" ", 215, 215, "  const events = await readAll(lane, { page: 64 });"], [" ", 216, 216, "  expect(events.length).toBeGreaterThan(256);"]],
      comment: { after: 2, who: "reviewer · pi-sol", at: "R1-review2", text: "Fixture-only change: 256 → 128 inputs. Every assertion, the 512 ring and the production event/frame bounds are unchanged; the original red on f34f3df6 stays in history." } },
    { path: "packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.5.0.patch", add: 9, del: 4, hunk: D.HUNK.at, fromHunk: true },
    { path: "packages/corral-pi/src/host/queue.ts", add: 212, del: 18, collapsed: true },
    { path: "packages/corral-pi/src/bridge/queue.ts", add: 88, del: 3, collapsed: true },
    { path: "docs/guide/pi-native.md", add: 24, del: 2, collapsed: true },
  ];
  const VERDICTS = [
    ["gate", "fail", "f34f3df6", "04:45", "fail · cross-lane paging fixture overflow (256 inputs → 528 events, 512 ring)", true],
    ["gate", "done", "85b45e1e", "08:26", "pass · quick tier · receipt 77b8a3c1", false],
    ["review", "done", "85b45e1e", "22:40", "approve · R1-review2 · notes c4328f8e", false],
    ["you", "next", "85b45e1e", "", "your verdict", false],
  ];

  const wordMark = (s, w, cls) => (w ? esc(s).replace(esc(w), `<mark class="${cls}">${esc(w)}</mark>`) : esc(s));
  function hunkLines(f) {
    if (f.fromHunk) { let o = 1990, n = 1990; return D.HUNK.lines.map(([k, s]) => [k, k === "+" ? null : o++, k === "-" ? null : n++, s]); }
    return f.lines;
  }
  function unified(f, S) {
    const ls = hunkLines(f);
    let out = `<div class="hh">${esc(f.hunk)}</div>`;
    ls.forEach((l, i) => {
      const [k, o, n, s, w] = l;
      out += `<div class="ln ${k === "+" ? "add" : k === "-" ? "del" : ""}"><span>${o ?? ""}</span><span>${n ?? ""}</span><span class="s">${k === " " ? "" : k}</span><span class="code">${wordMark(s, w, k === "+" ? "a" : "d")}</span></div>`;
      if (f.comment && f.comment.after === i) out += commentHtml(f.comment);
    });
    return `<div class="diff">${out}</div>`;
  }
  function split(f) {
    const ls = hunkLines(f);
    const rows = []; let i = 0;
    while (i < ls.length) {
      if (ls[i][0] === " ") { rows.push([ls[i], ls[i], i]); i++; continue; }
      const dels = [], adds = [];
      while (i < ls.length && ls[i][0] === "-") dels.push(ls[i++]);
      while (i < ls.length && ls[i][0] === "+") adds.push(ls[i++]);
      for (let j = 0; j < Math.max(dels.length, adds.length); j++) rows.push([dels[j], adds[j], i - 1]);
    }
    let out = `<div class="hh">${esc(f.hunk)}</div>`;
    rows.forEach(([l, r, idx]) => {
      const side = (x, which) => (x ? `<div class="${x[0] === "-" ? "del" : x[0] === "+" ? "add" : ""}"><span>${(which ? x[2] : x[1]) ?? ""}</span><span class="s" style="color:${x[0] === "+" ? "var(--ok)" : x[0] === "-" ? "var(--fail)" : "var(--ink-4)"}">${x[0] === " " ? "" : x[0]}</span><span class="code" style="overflow:hidden;text-overflow:ellipsis">${wordMark(x[3], x[4], x[0] === "+" ? "a" : "d")}</span></div>` : `<div class="e"><span></span><span></span><span></span></div>`);
      out += `<div class="row2">${side(l, 0)}${side(r, 1)}</div>`;
      if (f.comment && f.comment.after === idx && (r && r[0] === "+")) out += commentHtml(f.comment);
    });
    return `<div class="diff split">${out}</div>`;
  }
  const commentHtml = (c) => `<div class="cm"><div class="cmb"><div class="who"><b>${esc(c.who)}</b> · ${esc(c.at)} · on 85b45e1e</div>${esc(c.text)}</div>
    <div class="cmb" style="margin-top:6px;display:flex;gap:8px;align-items:center;box-shadow:inset 0 0 0 1px var(--rule)"><span class="dim" style="flex:1">Reply or add a line comment as owner…</span><span class="kbd">c</span></div></div>`;

  function filesHtml(S, phone) {
    return FILES.map((f) => `<div style="margin-bottom:14px"><div class="ev-file" style="font-size:11.5px"><span class="p" style="color:var(--ink-2)">${esc(f.path)}</span><span class="c-ok">+${f.add}</span><span class="c-fail">−${f.del}</span>${f.illus ? illus("lines illustrative") : ""}${f.comment ? `<button class="btn quiet" style="margin-left:auto;height:22px;font-size:11.5px" data-act="ask-open" data-chip="hunk">${ic("chat")}Ask about this hunk</button>` : ""}</div>
      ${f.collapsed ? `<button class="diff" style="display:block;width:100%;text-align:left;padding:8px 12px;color:var(--ink-3);font-family:var(--font);font-size:12px" data-act="toast" data-msg="Expanded ${esc(f.path)}">${ic("chev")} ${f.add + f.del} changed lines collapsed · expand</button>` : S.review.layout === "split" && !phone ? split(f) : unified(f, S)}</div>`).join("");
  }
  const verdicts = (phone) => `<div style="display:grid;gap:${phone ? 8 : 5}px">${VERDICTS.map(([k, st, sha, at, txt, stale]) => `<div style="display:grid;grid-template-columns:${phone ? "16px 1fr" : "16px 56px 78px 44px minmax(0,1fr)"};gap:8px;align-items:baseline;font-size:12.5px;${stale ? "opacity:.8" : ""}">
      ${mk(stale ? "stale" : st)}${phone ? `<span><b>${k}</b> · <span class="mono">${sha}</span> ${at} ${stale ? `<span class="chip held">older candidate</span>` : ""}<br><span class="${stale ? "dim" : ""}" style="${stale ? "text-decoration:line-through" : ""}">${esc(txt)}</span></span>` : `<b>${k}</b><span class="mono ${stale ? "c-held" : ""}">${sha}</span><span class="mono dim">${at}</span><span class="${stale ? "dim" : k === "you" ? "c-you" : ""}">${stale ? `<s>${esc(txt)}</s> <span class="chip held">stale · older candidate</span>` : esc(txt)}</span>`}</div>`).join("")}</div>`;
  const report = `<div class="plan-doc" style="max-width:72ch"><h3 style="margin-top:0">T-868 implementation report · rendered</h3><p>Implemented the Pi 1.5.0 host queue ledger with stable bound identities, synchronous pre-mutation lifecycle persistence, bounded bridge inspection/withdrawal, canonical operation replay and pane-only interactive edits. Delivery observation remains separate from execution and turn completion.</p>
    <h3>Acceptance</h3><ul><li>${mk("done")} Targeted withdrawal and both start races with exact-event barriers</li><li>${mk("done")} Replay, conflict, operation bound, foreign/stale targets leave the queue unchanged</li><li>${mk("done")} Pane edit, persistence, reload, rich Esc/dequeue and plain restore</li><li>${mk("done")} Package/host/bridge/composer checks on Pi 1.5.0; compat lock hash agrees</li></ul>
    <div class="dim" style="font-size:12px;margin-top:10px">Source: <span class="mono">artifacts/T-868/R1/1c6d848a-T-868-implementation-report.md</span> and run result (real text, condensed)</div></div>`;
  const shot = `<div class="term" style="width:100%;max-width:640px;white-space:pre;font-size:11px"><span class="w8">pi · bound session · plain mode · 140x46</span>\n\n<span class="p">›</span> draft kept: "tighten the withdraw race test"\n\n<span class="w8">Esc</span>  aborted turn · restored 1 pane item\n     <span class="ok">external identities retained: 2</span>\n     queued  1 steering · 1 follow-up\n\n<span class="p">›</span> tighten the withdraw race test<span class="cur"></span></div><div class="dim" style="font-size:12px;margin-top:6px">artifacts/T-868/R1/838f50e2-140x46-esc.ansi · "140x46 real plain-mode terminal after Esc abort with external items retained" (title real, content illustrative)</div>`;

  SCREENS.review = {
    desk(S) {
      const list = [["T-868", "host-queue-ledger", "85b45e1e", "gate ✓ · review ✓ · you", "you"], ["T-890", "pm-bounded-read-contract", "1751bd2", "quick gate running", "corral"], ["T-894", "api-connections", "6b36916", "gate running", "corral"], ["T-867", "queue-contract-core", "5949a4b", "gate stale · re-gate", "held"]];
      const tab = S.review.tab || "diff";
      const tabs = [["diff", "Diff · 5 files"], ["report", "Report"], ["shot", "Screenshot"], ["log", "Gate log"]];
      const body = tab === "diff" ? filesHtml(S) : tab === "report" ? report : tab === "shot" ? shot : `<div class="term">${["$ scripts/gate.sh --tier quick  (85b45e1e)", "  build     corral-pi            12.4s", "  bun test  queue/bridge         16 pass", "  cross-lane private herdr        5 pass", "  receipt   77b8a3c1", "PASS"].map((l) => (/pass|PASS/.test(l) ? `<span class="ok">${esc(l)}</span>` : esc(l))).join("\n")}</div><div class="illus" style="margin-top:6px">log lines illustrative · receipt id real</div>`;
      return `${connBanner()}<div class="cols stalefx" style="grid-template-columns:300px minmax(0,1fr)">
        <div class="pane" data-scroll="rv-l"><div class="list"><div class="grp g-you"><span class="gl">Your verdict</span><span class="gn">1</span></div>
          ${list.map(([id, t, sha, st, a], i) => (i === 1 ? `<div class="grp g-corral"><span class="gl">Verifying</span><span class="gn">3</span></div>` : "") + `<button class="row" style="grid-template-columns:14px minmax(0,1fr);height:auto;padding:8px 14px 8px 16px" aria-selected="${i === 0}" data-act="toast" data-msg="${id} selected"><span class="actor a-${a}"></span><span style="min-width:0"><span class="mono dim" style="font-size:11.5px">${id} · ${sha}</span><span class="tt" style="display:block">${t}</span><span class="dim" style="font-size:12px">${st}</span></span></button>`).join("")}
          <div class="illus" style="padding:8px 16px">T-868 here is a replay of its 2026-11-08 review; the rest follow the snapshot.</div></div></div>
        <div class="pane" data-scroll="rv-r" style="display:flex;flex-direction:column">
          <div class="qhead"><div class="meta"><span class="mono">T-868 · R1 · candidate 85b45e1e</span><span>base main · 5 files · +334 −28</span></div><h1>host-queue-ledger</h1></div>
          <div style="padding:12px 24px;border-bottom:1px solid var(--rule)"><div class="h3" style="margin-top:0">Verdicts · bound to candidate SHA</div>${verdicts()}</div>
          <div style="display:flex;align-items:center;gap:10px;padding:8px 24px 0"><div class="tabs" style="flex:1;border:0">${tabs.map(([k, l]) => `<button role="tab" data-act="rtab" data-v="${k}" aria-selected="${tab === k}">${l}</button>`).join("")}</div>${tab === "diff" ? `<div class="seg"><button data-act="rlayout" data-v="split" aria-pressed="${S.review.layout === "split"}">Side by side</button><button data-act="rlayout" data-v="unified" aria-pressed="${S.review.layout === "unified"}">Unified</button></div>` : ""}</div>
          <div style="padding:12px 24px;flex:1;border-top:1px solid var(--rule)">${body}</div>
          <div class="decide"><button class="btn" data-act="confirm" data-kind="approve-review">Approve 85b45e1e</button><button class="btn ghost" data-act="toast" data-msg="Request changes: comments go to the worker as round 2 input">Request changes</button><span class="attr">Verdicts apply to <span class="mono">85b45e1e</span> only; a new candidate makes them stale. Approving needs a fresh confirm.<br><span class="mono">actor=owner · via=app:pawis-desktop</span></span></div>
        </div></div>`;
    },
    phone(S) {
      const tab = S.review.tab || "diff";
      const body = `<div class="pband"><div class="mono dim" style="font-size:12.5px">T-868 · R1 · 85b45e1e ${illus("replay")}</div><div style="font-size:19px;font-weight:700;margin-top:4px">host-queue-ledger</div></div>
        <div class="pband"><div class="bh">Verdicts<span class="r">bound to SHA</span></div>${verdicts(true)}</div>
        <div class="pseg">${[["diff", "Diff"], ["report", "Report"], ["shot", "Screen"]].map(([k, l]) => `<button data-act="rtab" data-v="${k}" aria-pressed="${tab === k}">${l}</button>`).join("")}</div>
        <div style="padding:4px 14px 20px">${tab === "diff" ? `<div class="dim" style="font-size:13px;margin:0 4px 8px">Unified on phones; side by side needs ≥ 900 pt.</div>${filesHtml(S, true)}` : tab === "report" ? report : shot}</div>`;
      return { title: "Review", backLabel: "Back", body, decide: `<button class="pbtn ghost" data-act="toast" data-msg="Request changes">Changes</button><button class="pbtn wide" data-act="confirm" data-kind="approve-review">Approve 85b45e1e</button>` };
    },
    keys(e, S) { if (e.key === "s") { S.review.layout = S.review.layout === "split" ? "unified" : "split"; return; } return false; },
  };
  Object.assign(ACTIONS, {
    rtab: (d) => { S.review.tab = d.v; render(); },
    rlayout: (d) => { S.review.layout = d.v; render(); },
  });
})();
