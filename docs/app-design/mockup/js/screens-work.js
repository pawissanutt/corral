// Inbox, question (handover sheet), Now workspace, Changes, phone task page, phone More.
(() => {
  const { esc, ic, mk, rail, row, prow, money, illus, T } = U;
  const Q = D.Q;
  const qState = (S) => (S.data === "elsewhere" ? "elsewhere" : S.q.state);
  const offline = (S) => ["offline", "down"].includes(S.health);

  // ---------- handover bands ----------
  function band(letter, word, body, extra = "") {
    return `<section class="band" ${extra}><div class="m"><b>${letter}</b><span>${word}</span></div><div class="c">${body}</div></section>`;
  }
  const waitsHtml = () => `<div class="waits">${Q.waits.map(([id, txt, k]) => `<div class="wait">${k === "you" ? `<span class="dot c-you"></span>` : k === "plan" ? mk("next") : mk("na")}<span class="id">${id}</span><span class="${k === "info" ? "dim" : ""}">${esc(txt)}</span></div>`).join("")}</div>`;

  function situation(S, phone) {
    const st = qState(S);
    const stamp = st === "elsewhere" ? `<div class="notice ok" style="margin-top:12px"><span class="stamp">ANSWERED ELSEWHERE</span><span>Answered on the <b>PM pane</b> at ${Q.answeredElsewhere.at} with option 1 · <span class="mono">via=pm-pane</span>. This copy is read-only; one answer wins across devices and the PM pane.</span></div>`
      : st === "mine" ? `<div class="notice ok" style="margin-top:12px"><span class="stamp">ANSWERED</span><span>By you at 22:12 with option ${S.q.choice + 1} · <span class="mono">actor=owner via=app:${S.frame === "phone" ? "pawis-iphone" : "pawis-desktop"}</span>. Other devices and the PM pane now show it answered.</span></div>`
      : st === "queued" ? `<div class="notice warn" style="margin-top:12px"><span class="stamp void">QUEUED</span><span>Option ${S.q.choice + 1} waits on this device. It sends when corrald is reachable, only if rev 2 is still current; otherwise it comes back to you.</span></div>` : "";
    if (phone) return `<p class="lede">${esc(Q.text)}</p><div class="h3" style="margin:14px 0 6px">Waiting on this answer</div>${waitsHtml()}${stamp}`;
    return `<div style="display:grid;grid-template-columns:minmax(0,1fr) 290px;gap:22px"><p class="lede">${esc(Q.text)}</p><div><div class="h3" style="margin-top:2px">Waiting on this answer</div>${waitsHtml()}</div></div>${stamp}`;
  }

  function miniGraph(phone) {
    const w = phone ? 340 : 300, h = 118;
    const N = (x, y, id, t, cls, sub) => `<g class="node ${cls}"><rect x="${x}" y="${y}" width="${phone ? 98 : 86}" height="34" rx="5"/><text x="${x + 8}" y="${y + 14}">${id}</text><text x="${x + 8}" y="${y + 27}" class="lbl">${sub}</text></g>`;
    const cw = phone ? 117 : 106;
    return `<svg class="minigraph" viewBox="0 0 ${w} ${h}" role="img" aria-label="Position in plan T-870: T-701 done, T-867 stale gate, T-868 needs you, T-869 queued">
      <path d="M${phone ? 98 : 86} 25 H${cw}"/><path d="M${cw + (phone ? 98 : 86)} 25 C${cw + 104} 25, ${cw + 104} 25, ${2 * cw} 25"/><path d="M${cw + (phone ? 98 : 86)} 25 C${2 * cw - 8} 25, ${2 * cw - 14} 82, ${2 * cw} 82"/>
      ${N(0, 8, "T-701", "", "", "plan P2 ✓")}${N(cw, 8, "T-867", "", "", "gate ◌ stale")}${N(2 * cw, 8, "T-868", "", "you", "needs you")}${N(2 * cw, 65, "T-869", "", "dim", "after T-867")}
      <text x="0" y="${h - 4}" class="lbl">T-870 · layer 1 of 2 · member 2 of 3 · final accept needs all 3</text></svg>`;
  }
  function place(S, phone) {
    const t = T("T-868");
    if (S.data === "loading") return `<div style="display:grid;gap:8px"><div class="skel" style="width:90%"></div><div class="skel" style="width:70%"></div><div class="skel" style="width:80%;height:60px"></div><span class="illus">Task detail loads lazily from corrald; the question itself came with the board projection.</span></div>`;
    const goal = `<p class="goaltext"><b>T-868 host-queue-ledger</b> · ${esc(t.goal)}</p><div class="dim" style="font-size:12px">Plan <span class="mono">T-870</span> Pi input: stable queue item lifecycle · governed by <span class="mono">T-701@P2</span> · run R1 on pi-sol xhigh, step 3 of 5</div>`;
    return phone ? `${goal}<div style="margin-top:12px">${miniGraph(true)}</div>` : `<div class="place"><div>${goal}</div><div>${miniGraph(false)}</div></div>`;
  }

  const hunkHtml = (phone) => {
    let o = 1990, n = 1990, out = "";
    D.HUNK.lines.forEach(([k, s]) => {
      const cls = k === "+" ? "add" : k === "-" ? "del" : "";
      const ln = k === "-" ? [o++, ""] : k === "+" ? ["", n++] : [o++, n++];
      out += `<div class="ln ${cls}"><span>${ln[0]}</span><span>${ln[1]}</span><span class="s">${k === " " ? "" : k}</span><span class="code">${esc(s)}</span></div>`;
    });
    return `<div class="diff"><div class="hh">${esc(D.HUNK.at)}</div>${out}</div>`;
  };
  function evidence(S, phone) {
    if (S.data === "loading") return `<div style="display:grid;gap:6px"><div class="skel" style="height:120px"></div><span class="illus">Evidence streams in after the sheet renders.</span></div>`;
    if (S.data === "error") return `<div class="notice bad"><b>Evidence did not load.</b> Artifact 9a87439d timed out from the artifact store. The options and consequences below come from the board projection and are current as of rev 2. <button class="btn quiet" data-act="toast" data-msg="Retrying evidence fetch…">Retry</button></div>`;
    const tabs = [["diff", "Diff hunk"], ["file", "File excerpt"], ["log", "Gate log tail"], ["shot", "Screenshot"]];
    const t = `<div class="ev-tabs" role="toolbar" aria-label="Evidence">${tabs.map(([k, l]) => `<button data-act="setq" data-k="ev" data-v="${k}" aria-pressed="${S.q.ev === k}">${l}</button>`).join("")}</div>`;
    let body = "";
    if (S.q.ev === "diff") body = `<div class="ev-file"><span class="p">${esc(D.HUNK.file)}</span></div>${hunkHtml(phone)}<div class="dim" style="font-size:11.5px;margin-top:6px">With option 1, <span class="mono">restoreInput</span> returns <span class="mono">undefined</span> for Corral/extension items, so <span class="mono">continue</span> keeps them queued.</div>`;
    if (S.q.ev === "file") body = `<div class="ev-file"><span class="p">${esc(D.EXCERPT.file)}</span></div><div class="term" style="white-space:pre-wrap">${D.EXCERPT.lines.map(esc).join("\n")}</div>`;
    if (S.q.ev === "log") body = `<div class="ev-file"><span class="p">${esc(D.LOGTAIL.file)}</span>${illus("tail illustrative · title real")}</div><div class="term">${D.LOGTAIL.lines.map((l) => (/PASS/.test(l) ? `<span class="ok">${esc(l)}</span>` : esc(l))).join("\n")}</div>`;
    if (S.q.ev === "shot") body = `<div class="ev-file"><span class="p">artifacts/T-868/R1/7bc302c8-52x24.ansi · rendered</span>${illus("content illustrative")}</div><div class="term" style="width:${phone ? "100%" : "420px"};white-space:pre;font-size:10.5px;line-height:1.45"><span class="w8">pi · bound session · T-868 R1</span>\n\n<span class="p">›</span> restore the queued edit to my draft\n\n<span class="w8">queued</span>  1 steering · 2 follow-ups\n<span class="w8">Esc</span>     restored 1 pane item to the editor\n        <span class="ok">kept 2 Corral/extension items queued</span>\n\n<span class="p">›</span> <span class="cur"></span></div><div class="dim" style="font-size:11.5px;margin-top:6px">52x24 plain-mode terminal after pane-only native dequeue (real artifact title).</div>`;
    return t + body;
  }

  function options(S, phone) {
    const st = qState(S);
    const locked = st !== "open" && st !== "queued";
    const chosen = st === "elsewhere" ? Q.answeredElsewhere.option : S.q.choice;
    const opt = (o, i) => `<button class="opt" role="radio" aria-checked="${chosen === i}" data-act="setq" data-k="choice" data-v="${i}" ${locked ? "disabled" : ""}>
        <span class="num">${i + 1}</span>
        <span class="ol">${esc(o.label)}${o.rec ? `<span class="rec">worker recommends</span>` : ""}</span>
        <dl class="cons"><dt>Then</dt><dd>${esc(o.next)}</dd><dt>Unblocks</dt><dd>${esc(o.unblocks)}</dd><dt>Costs</dt><dd>${esc(o.costs)} <a class="proof" data-act="setq" data-k="ev" data-v="${i === 0 ? "diff" : "file"}">see ${esc(o.proof)}</a></dd></dl>
      </button>`;
    return `<div class="opts" role="radiogroup" aria-label="Options">${Q.options.map(opt).join("")}
      <label class="opt free" ${locked ? 'style="opacity:.5"' : ""}><span class="num">✎</span><textarea data-bind="qdraft" placeholder="Write a different answer…" ${locked ? "disabled" : ""}>${esc(S.q.draft)}</textarea></label></div>
      <div class="dim" style="font-size:12px;margin-top:2px"><b class="ink2">Why the worker recommends 1:</b> ${esc(Q.why)}</div>`;
  }

  function thread(S, phone) {
    return `${Q.thread.map((m) => `<div class="msg"><div class="who"><b>${esc(m.who)}</b><span>${m.at}</span>${m.via ? `<span class="mono">${m.via}</span>` : ""}</div><div>${esc(m.text)}</div></div>`).join("")}
      <div class="reply"><textarea placeholder="Ask worker T-868 before answering…" aria-label="Follow-up to the asker"></textarea><button class="btn ghost" data-act="toast" data-msg="Sent to worker T-868 · posted to the question thread · actor=owner">${ic("send")}Ask</button></div>
      <div class="illus" style="margin-top:6px">thread messages illustrative</div>`;
  }
  function history() {
    return Q.history.map((h) => `<div class="hist"><span class="bar ${h.who === "you" ? "you" : ""}"></span><span class="at">${h.at} · ${h.who === "you" ? "your decision" : "tried"}</span><span>${esc(h.text)}</span></div>`).join("");
  }

  function decideBar(S, phone) {
    const st = qState(S), off = offline(S);
    const n = S.q.draft ? "your text" : `option ${S.q.choice + 1}`;
    if (st === "elsewhere" || st === "mine") {
      return phone ? `<button class="pbtn ghost wide" data-act="ptab" data-to="inbox">Back to Inbox</button><button class="pbtn wide" data-act="next-q">Next · T-885 plan</button>`
        : `<span class="stamp">${st === "elsewhere" ? "ANSWERED ON PM PANE 22:14" : "ANSWERED 22:12"}</span><button class="btn ghost" data-act="goto" data-to="session">Watch T-868 resume</button><span class="attr">One answer wins: the PM pane, this app and every paired device read the same question record.</span>`;
    }
    const label = off ? `Queue answer · ${n}` : `Answer · ${n}`;
    if (phone) return `<button class="pbtn ghost" data-act="thread">${ic("chat")}Ask worker</button><button class="pbtn wide" data-act="answer">${label}</button>`;
    return `<button class="btn" data-act="answer">${label}<span class="kbd">Ctrl ↵</span></button><button class="btn ghost" data-act="toast" data-msg="Focus moved to the thread reply box">Ask the worker</button><button class="btn quiet" data-act="toast" data-msg="Opened in the PM conversation">Discuss with PM</button>
      <span class="attr">${off ? `Offline: answers queue with their revision; plan accepts and grants refuse.` : `Runs <span class="mono">corral question answer --owner ${Q.key} --rev 2</span>`}<br><span class="mono">actor=owner · via=app:pawis-desktop</span></span>`;
  }

  function qhead(S, phone) {
    return `<div class="qhead"><div class="meta"><span class="chip you">needs you</span><span class="chip">${Q.kind}</span><span class="chip fail">blocking</span><span class="mono">${Q.key} · rev ${Q.rev}</span><span>from ${Q.from} · ${Q.via} · ${Q.age} ago</span></div><h1>${esc(Q.short)}</h1></div>`;
  }

  function sheetDesk(S, withSide) {
    const st = qState(S);
    // Focus mode (withSide): S, A, R in the centre; B, T, H in the context column, so the options sit in the first viewport.
    const bands = `${qhead(S)}
      ${band("S", "Situation", situation(S))}
      ${withSide ? "" : band("B", "Background", place(S))}
      ${band("A", "Assessment", `<div class="evbox">${evidence(S)}</div>`)}
      ${band("R", "Recommendation", options(S))}
      ${withSide ? "" : band("T", "Thread", thread(S)) + band("H", "History", history())}`;
    return `<div class="${st !== "open" && st !== "queued" ? "answered" : ""}" style="display:flex;flex-direction:column;min-height:100%">${bands}<div style="flex:1"></div><div class="decide">${decideBar(S)}</div></div>`;
  }

  // ---------- Inbox ----------
  const needYou = (S) => {
    const st = qState(S);
    const items = [];
    if (S.data !== "empty") items.push({ k: "q", lead: "Worker asks", id: Q.key, title: Q.short, meta: `T-868 blocked · ${Q.age} · owner only`, state: st });
    items.push({ k: "plan", lead: "Plan awaits acceptance", id: "T-885 P2", title: "PM supervisor evolution: bounded reads", meta: "reviewed · approve P2 · 3 members" });
    items.push({ k: "pm", lead: "PM asks", id: "PM/Q7", title: "T-852 repair budget: re-gate only, or one more round?", meta: "12m · nothing blocked", sample: true });
    return S.data === "empty" ? [] : items.concat(ASKUI.inboxItems());
  };

  function inboxList(S) {
    if (S.data === "loading") return `<div class="list">${[1, 2, 3, 4, 5].map(() => `<div style="padding:10px 16px;display:grid;gap:6px"><div class="skel" style="width:40%"></div><div class="skel" style="width:85%"></div></div>`).join("")}</div>`;
    const errTop = S.data === "error" ? `<div class="err" style="margin:10px 12px"><span><b>Couldn't refresh the inbox.</b> corrald returned an error on <span class="mono">sub.open inbox</span>. Showing the last state from 22:08; retrying every 10 s. <button class="btn quiet" data-act="toast" data-msg="Retrying…">Retry now</button></span></div>` : "";
    const items = needYou(S);
    let h = `${errTop}<div class="grp g-you"><span class="gl">Needs you</span><span class="gn">${items.length}</span></div>`;
    if (!items.length) h += `<div class="empty"><h3>Nothing needs you.</h3><p>Corral is verifying 3, workers are producing 2, 23 are queued. You'll get a push if that changes.</p><button class="btn ghost" data-act="goto" data-to="now">Open Now</button></div>`;
    items.forEach((it) => {
      const sel = S.inboxSel === it.k;
      h += `<button class="row" style="grid-template-columns:14px minmax(0,1fr) auto;height:auto;padding:9px 14px 9px 16px;align-items:start" data-act="inbox-sel" data-k="${it.k}" aria-selected="${sel}">
        <span class="actor a-you" style="margin-top:5px;${it.state && it.state !== "open" ? "background:var(--ink-4)" : ""}"></span>
        <span style="min-width:0"><span class="dim" style="font-size:11.5px">${it.lead} · <span class="mono">${it.id}</span>${it.sample ? " · " + illus("sample") : ""}</span><span class="tt" style="display:block;font-weight:600;margin:1px 0">${esc(it.title)}</span><span class="dim" style="font-size:12px">${it.state === "elsewhere" ? "answered on PM pane 22:14" : it.state === "mine" ? "answered by you 22:12" : it.meta}</span></span>
        <span class="mono dim" style="font-size:11px">${it.age || (it.k === "q" ? Q.age : it.k === "plan" ? "10h" : "12m")}</span></button>`;
    });
    h += `<div class="grp" style="margin-top:6px"><span class="gl" style="font-weight:600">Since your last visit</span><span class="gn">${D.LAST_VISIT}</span></div><div class="feed">${feed(S, 8)}</div>
      <button class="more" data-act="goto" data-to="changes">All 12 changes ${ic("chev")}</button>`;
    return `<div class="list" role="listbox" aria-label="Inbox">${h}</div>`;
  }
  function feed(S, n = 99, phone) {
    let h = "";
    D.CHANGES.slice(0, n).forEach(([t, id, k, txt]) => {
      const m = k === "start" ? `<span style="font-size:10px;color:var(--worker)">▶</span>` : k === "dot" ? mk("na") : k === "msg" ? `<span class="dot c-you" style="width:6px;height:6px"></span>` : mk(k);
      h += phone ? `<button class="prow" style="min-height:48px;grid-template-columns:44px minmax(0,1fr) 14px" data-act="open-task" data-id="${id}"><span class="mono dim" style="font-size:12.5px">${t}</span><span style="font-size:14.5px;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap"><span class="mono dim">${id}</span> ${esc(txt)}</span>${m}</button>`
        : `<button class="ev unseen" data-act="open-task" data-id="${id}"><span class="t">${t}</span>${m}<span class="i">${id}</span><span class="x">${esc(txt)}</span></button>`;
    });
    return h;
  }

  function planPreview() {
    const p = D.PLANS[0];
    return `<div class="qhead"><div class="meta"><span class="chip you">needs you</span><span class="chip ok">reviewed · approve P2</span><span class="mono">T-885 P2</span></div><h1>${esc(p.title)}</h1></div>
      <div style="padding:16px 24px" class="plan-doc"><p>${esc(p.goal)}</p><h3>What accepting does</h3><p>Publishes 3 member tasks to <b>main</b> delivery: ${p.members.map((m) => `<span class="mono">${m[0]}</span>`).join(" → ")}. Autopilot may then admit the first one.</p>
      <div style="margin-top:16px;display:flex;gap:8px"><button class="btn" data-act="plan-open" data-id="T-885">Open plan review</button></div></div>`;
  }

  SCREENS.inbox = {
    desk(S) {
      const right = S.data === "loading" ? `<div style="padding:24px;display:grid;gap:10px"><div class="skel" style="width:30%"></div><div class="skel" style="width:70%;height:18px"></div><div class="skel" style="height:90px"></div></div>`
        : S.data === "empty" ? `<div class="empty" style="padding:60px 40px"><h3>Inbox is clear</h3><p>Answered questions and accepted plans stay in Changes and Timeline. Nothing here is waiting on you.</p></div>`
        : S.inboxSel === "plan" ? planPreview()
        : S.inboxSel === "pm" ? `<div class="qhead"><div class="meta"><span class="chip you">needs you</span><span class="mono">PM/Q7</span>${illus("sample question")}</div><h1>T-852 repair budget: re-gate only, or one more round?</h1></div><div style="padding:16px 24px"><p class="lede" style="font-size:14px;max-width:70ch">T-852 has spent $43 (warning at $20) over 2 review rounds. Its gate went stale because the base moved, not because of a code defect.</p></div>`
        : S.inboxSel === "perm" || S.inboxSel === "conf" ? ASKUI.inboxPane(S.inboxSel)
        : sheetDesk(S, false);
      return `${connBanner()}<div class="cols stalefx" style="grid-template-columns:400px minmax(0,1fr)"><div class="pane" data-scroll="inbox-l">${inboxList(S)}</div><div class="pane" data-scroll="inbox-r">${right}</div></div>`;
    },
    phone(S) {
      let body = "";
      if (S.data === "loading") body = `<div style="padding:12px 14px;display:grid;gap:10px">${[1, 2].map(() => `<div style="padding:16px;border-radius:14px;box-shadow:inset 0 0 0 1px var(--rule);display:grid;gap:8px"><div class="skel" style="width:40%"></div><div class="skel" style="width:90%;height:16px"></div><div class="skel" style="width:60%"></div></div>`).join("")}</div>`;
      else if (S.data === "error") body = `<div class="err"><span><b>Couldn't refresh.</b> corrald answered with an error on <span class="mono">board.subscribe</span>. Showing the last state from 22:08. Pull to retry.</span></div>` + phoneNeed(S);
      else body = phoneNeed(S);
      if (S.data !== "loading") body += `<div class="ph-sec">Since ${D.LAST_VISIT}<span class="n">12</span><button class="r" data-act="push" data-page="changes">All</button></div>${feed(S, 6, true)}`;
      return { title: "Inbox", large: true, body };
    },
    keys(e, S) {
      const order = ["q", "plan", "pm", "perm", "conf"];
      if (e.key === "j" || e.key === "ArrowDown") { S.inboxSel = order[Math.min(4, order.indexOf(S.inboxSel) + 1)]; return; }
      if (e.key === "k" || e.key === "ArrowUp") { S.inboxSel = order[Math.max(0, order.indexOf(S.inboxSel) - 1)]; return; }
      if (e.key === "Enter" || e.key === "f") { if (S.inboxSel === "q") S.screen = "question"; else if (S.inboxSel === "plan") { S.plan = "T-885"; S.screen = "plans"; } return; }
      if (S.inboxSel === "q" && (e.key === "1" || e.key === "2")) { S.q.choice = +e.key - 1; return; }
      return false;
    },
  };
  function phoneNeed(S) {
    const items = needYou(S);
    if (!items.length) return `<div class="empty"><h3>Nothing needs you.</h3><p>Corral is verifying 3 · workers producing 2 · 23 queued. You'll get a push when that changes.</p><button class="pbtn ghost" data-act="ptab" data-to="now">Open Now</button></div>`;
    const st = qState(S);
    const q = items[0];
    return `<div class="ph-sec" style="color:var(--you)">Needs you<span class="n">${items.length}</span></div>
      <button class="pcard-q" data-act="push" data-page="question"><span class="qk"><span class="dot"></span>Worker asks · ${Q.age} · blocking</span><div class="qt">${esc(Q.short)}</div>
        <div class="qm">${st === "elsewhere" ? "Answered on PM pane 22:14 · option 1" : st === "mine" ? "Answered by you · option " + (S.q.choice + 1) : `T-868 host-queue-ledger · 2 options · worker recommends 1`}</div></button>
      <button class="pcard-q" data-act="plan-open" data-id="T-885"><span class="qk"><span class="dot"></span>Plan awaits acceptance · 10h</span><div class="qt">T-885 P2 · PM supervisor evolution: bounded reads</div><div class="qm">Reviewer approved P2 · accepting publishes 3 members</div></button>
      <button class="pcard-q" data-act="toast" data-msg="Sample question (illustrative)"><span class="qk"><span class="dot"></span>PM asks · 12m · ${illus("sample")}</span><div class="qt">T-852 repair budget: re-gate only, or one more round?</div><div class="qm">Nothing blocked · PM keeps other work moving</div></button>
      ${ASKUI.inboxItems().map((a) => `<button class="pcard-q" data-act="ask-open-thread"><span class="qk"><span class="dot"></span>${a.lead} · ${a.age}</span><div class="qt">${esc(a.title)}</div><div class="qm">${esc(a.meta)} · answers in the quick thread</div></button>`).join("")}`;
  }

  // ---------- Question (focus) ----------
  SCREENS.question = {
    desk(S) {
      const side = `<div class="side-col pane" data-scroll="q-side"><div class="h3" style="margin-top:0"><b style="font-size:13px;color:var(--ink-2)">B</b> Background · place <span class="r">T-870</span></div>${place(S, true)}<div class="h3"><b style="font-size:13px;color:var(--ink-2)">T</b> Thread with the asker <span class="r">${Q.thread.length}</span></div>${thread(S)}<div class="h3"><b style="font-size:13px;color:var(--ink-2)">H</b> History <span class="r">tried · your decisions</span></div>${history()}</div>`;
      return `${connBanner()}<div class="pagehead"><button class="btn quiet" data-act="goto" data-to="inbox">${ic("back")}Inbox</button><span class="sub">1 of 5 needing you</span><span class="acts"><span class="dim" style="font-size:12px">Keys <span class="kbd">1</span> <span class="kbd">2</span> choose · <span class="kbd">Ctrl ↵</span> answer · <span class="kbd">Esc</span> back</span><button class="btn ghost" data-act="ask-open" data-chip="T-868/R1/Q1">${ic("chat")}Ask about this</button><button class="btn ghost" data-act="next-q">Next · T-885 plan ${ic("chev")}</button></span></div>
        <div class="cols stalefx" style="grid-template-columns:minmax(0,1fr) 340px"><div class="pane" data-scroll="q-main">${sheetDesk(S, true)}</div>${side}</div>`;
    },
    phone(S) {
      const st = qState(S);
      const body = `<div class="pager"><span>1 of 5 needing you</span><button class="r" data-act="next-q">Next ${ic("chev")}</button></div>
        <div class="pband"><div class="meta" style="display:flex;gap:6px;flex-wrap:wrap;margin-bottom:8px"><span class="chip you">needs you</span><span class="chip fail">blocking</span><span class="chip">owner only</span><span class="mono dim" style="font-size:12px">${Q.key} · rev 2</span></div>
          <div style="font-size:20px;font-weight:700;line-height:1.25;letter-spacing:-.01em">${esc(Q.short)}</div><div class="dim" style="font-size:13px;margin-top:4px">from ${Q.from} · ${Q.via} · ${Q.age} ago</div></div>
        <div class="pband"><div class="bh"><b>S</b>Situation</div>${situation(S, true)}</div>
        <div class="pband"><div class="bh"><b>R</b>Options<span class="r">consequence of each</span></div>${options(S, true)}</div>
        <div class="pband"><div class="bh"><b>A</b>Evidence</div>${evidence(S, true)}</div>
        <div class="pband"><div class="bh"><b>B</b>Place</div>${place(S, true)}</div>
        <button class="pband" style="display:block;width:100%;text-align:left" data-act="thread"><div class="bh"><b>T</b>Thread with the asker<span class="r">${Q.thread.length} · Ask ›</span></div><div style="font-size:14.5px"><b>${esc(Q.thread[1].who)}:</b> ${esc(Q.thread[1].text)}</div></button>
        <div class="pband"><div class="bh"><b>H</b>History</div>${history()}</div>`;
      return { title: "Question", backLabel: "Inbox", body, decide: decideBar(S, true), right: `<button data-act="ask-open" data-chip="T-868/R1/Q1" aria-label="Ask the assistant about this">${ic("chat")}</button>` };
    },
    keys(e, S) {
      if (e.key === "1" || e.key === "2") { S.q.choice = +e.key - 1; return; }
      if (e.key === "j") { S.q.choice = 1; return; } if (e.key === "k") { S.q.choice = 0; return; }
      if (e.key === "e") { const o = ["diff", "file", "log", "shot"]; S.q.ev = o[(o.indexOf(S.q.ev) + 1) % 4]; return; }
      return false;
    },
  };
  Object.assign(window.ACTIONS, {
    "inbox-sel": (d) => { S.inboxSel = d.k; render(); },
    answer: () => {
      if (qState(S) !== "open") return;
      const off = offline(S);
      S.q.state = off ? "queued" : "mine";
      toast(off ? `Queued: ${Q.key} · option ${S.q.choice + 1} · sends on reconnect if rev 2 is current` : `Answered ${Q.key} · option ${S.q.choice + 1} · actor=owner via=app:${S.frame === "phone" ? "pawis-iphone" : "pawis-desktop"}`);
    },
    thread: () => { S.sheet = { title: "Thread · worker T-868", body: () => thread(S, true) }; render(); },
    "next-q": () => { S.plan = "T-885"; if (S.frame === "phone") { S.ptab = "plans"; S.pstack = [{ page: "plan" }]; render(); } else nav("plans"); },
    "plan-open": (d) => { S.plan = d.id; if (S.frame === "phone") { S.ptab = "plans"; S.pstack = [{ page: "plan" }]; render(); } else nav("plans"); },
  });

  // ---------- Now ----------
  function groupsHtml(S) {
    if (S.data === "loading") return `<div class="list">${Array.from({ length: 12 }, (_, i) => `<div style="display:grid;grid-template-columns:50px 1fr 64px 120px;gap:10px;padding:9px 16px"><div class="skel"></div><div class="skel" style="width:${60 + (i * 13) % 35}%"></div><div class="skel"></div><div class="skel"></div></div>`).join("")}</div>`;
    let h = `<div class="colhead" style="grid-template-columns:10px 14px 50px minmax(0,1fr) 64px 150px 34px 46px;column-gap:8px"><span></span><span></span><span>id</span><span>task</span><span title="run · gate · review · land">r g v l</span><span>now</span><span style="text-align:right">age</span><span style="text-align:right">cost</span></div>`;
    D.GROUPS.forEach((g) => {
      if (S.data === "empty" && g.key === "you") return;
      const n = g.key === "queued" ? 23 : g.key === "parked" ? 32 : g.ids.length + (g.extra ? g.extra.length : 0);
      h += `<div class="grp g-${g.key}"><span class="gl">${g.label}</span><span class="gn">${n}</span>${g.key === "corral" ? `<span class="gx">gating · reviewing · landing</span>` : ""}</div>`;
      if (g.key === "you" && qState(S) === "open") {
        h += row("T-868", S.sel);
        g.extra.forEach((x) => (h += `<button class="row r-you" data-act="${x.kind === "plan" ? "plan-open" : "toast"}" data-id="${x.id}" data-msg="Sample question (illustrative)"><span>${x.kind === "plan" ? '<span class="delta"></span>' : ""}</span><span class="actor a-you" style="border-radius:2px;${x.kind === "plan" ? "transform:rotate(45deg) scale(.8)" : ""}"></span><span class="id">${x.id.replace("PM/", "")}</span><span class="tt">${esc(x.title)}</span><span class="dim" style="font-size:11.5px">${x.kind === "plan" ? "plan" : "PM ask"}</span><span class="nw ${x.kind === "plan" ? "c-ok" : ""}" style="${x.kind === "plan" ? "color:var(--ok)" : ""}">${esc(x.note)}</span><span class="ag">${x.kind === "plan" ? "10h" : "12m"}</span><span></span></button>`));
      } else if (g.key === "you") {
        g.extra.forEach((x) => (h += `<button class="row r-you" data-act="plan-open" data-id="${x.id}"><span></span><span class="actor a-you"></span><span class="id">${x.id.replace("PM/", "")}</span><span class="tt">${esc(x.title)}</span><span></span><span class="nw">${esc(x.note)}</span><span class="ag"></span><span></span></button>`));
      }
      g.ids.filter((id) => g.key !== "you").forEach((id) => (h += row(id, S.sel)));
      if (g.more) h += `<button class="more" data-act="toast" data-msg="Expanded: 15 more queued, collapsed by reason (capacity)">+ ${g.more} more queued · capacity ${ic("chev")}</button>`;
      if (g.parked) h += `<button class="row r-parked" data-act="toast" data-msg="Parked: ${g.parked.n} tasks with your dispatch hold"><span></span><span class="actor a-parked"></span><span class="id">${g.parked.n}</span><span class="tt">${g.parked.split}</span><span></span><span class="nw" style="color:var(--ink-3)">held by you · not counted</span><span></span><span></span></button>`;
    });
    return `<div class="list" role="listbox" aria-label="Who acts next">${h}</div>`;
  }

  function storyHtml(S, id, phone) {
    const t = T(id);
    if (!t.steps) return `<div class="insp"><span class="mono dim">${id}</span><h2>${esc(t.title)}</h2><div class="goal">${t.plan ? `Plan ${t.plan} · ` : ""}${esc(t.now)}</div>
      <dl class="kv"><dt>Waits on</dt><dd>${(t.needs || []).map(([n, k]) => `<span class="mono">${n}</span> ${mk(k)}`).join(" · ") || "capacity"}</dd><dt>Unblocks</dt><dd>${(t.unblocks || []).map((u) => `<span class="mono">${u}</span>`).join(" ") || "—"}</dd><dt>You</dt><dd class="c-ok">nothing needed</dd></dl></div>`;
    const sha = t.gate?.sha || ({ "T-890": "1751bd2", "T-894": "6b36916", "T-867": "5949a4b", "T-852": "af7a6dc" })[id];
    const stg = [["run", t.rail[0], `${t.run || "R1"} · ${t.agent || ""}`], ["gate", t.rail[1], sha || "after run"], ["review", t.rail[2], "after gate"], ["land", t.rail[3], "main"]];
    const word = { done: "done", active: "now", fail: "failed", stale: "stale", next: "next", na: "n/a" };
    const isYou = t.actor === "you";
    return `<div class="insp">
      <div style="display:flex;gap:8px;align-items:center"><span class="mono dim">${id}</span>${t.plan ? `<span class="dim" style="font-size:12px">· plan ${t.plan}</span>` : ""}${t.changed ? `<span class="chip">▲ changed since ${D.LAST_VISIT}</span>` : ""}</div>
      <h2>${esc(t.title)}</h2><div class="goal">${t.machine ? `${t.agent} · on ${t.machine} ${illus("machine name illustrative")} · started ${t.started}` : ""}</div>
      <div class="stepper">${stg.map(([l, k, s]) => `<div class="${k === "active" ? "on" + (l === "run" ? (isYou ? " y" : " w") : "") : ""}"><div class="lab">${l}</div><div class="st">${mk(k, l === "run")}${isYou && l === "run" ? "waits on you" : word[k]}</div><div class="sm">${esc(s)}</div></div>`).join("")}</div>
      <div class="h3" style="margin-top:0">Progress <span class="r">${t.steps.filter((s) => s[0] === "done").length}/${t.steps.length}</span></div>
      <ul class="steps">${t.steps.map(([k, s]) => `<li class="${k}">${mk(k, true)}<span>${esc(s)}</span></li>`).join("")}</ul>
      <dl class="kv"><dt>Next</dt><dd><b>${t.next[0]}</b> · ${esc(t.next[1])}</dd>
        <dt>You</dt><dd>${isYou ? `<button class="btn" style="height:26px" data-act="goto" data-to="question">Answer T-868/R1/Q1</button>` : `<span class="c-ok">nothing needed</span>`}</dd>
        <dt>Needs</dt><dd>${(t.needs || []).map(([n, k]) => `<span class="mono">${n}</span> ${mk(k)}`).join(" &nbsp; ")}</dd>
        <dt>Unblocks</dt><dd>${(t.unblocks || []).map((u) => `<span class="mono">${u}</span>`).join(" ") || "—"}</dd>
        <dt>Cost</dt><dd>${t.cost ? money(t.cost) : "—"} · ${t.agent}${t.cost > 20 ? ` · <span class="c-held">over the $20 warning</span>` : ""}</dd></dl>
      ${phone ? "" : `<div style="display:flex;gap:8px;margin-top:16px;flex-wrap:wrap"><button class="btn ghost" data-act="open-session" data-id="${id}">${ic("chat")}Open session</button><button class="btn ghost" data-act="ask-open" data-chip="${id}">Ask about this</button><button class="btn quiet" data-act="goto" data-to="review">Review candidate</button></div>`}
    </div>`;
  }
  function liveTail(id) {
    const t = T(id), a = D.AGENTS.find((x) => x.task === id) || {};
    const tail = t.tail || a.tail || ["no live pane for this item"];
    return `<div style="padding:10px 12px"><div class="dim" style="font-size:11.5px;margin-bottom:6px;display:flex;gap:8px"><span class="mono">${t.pane || "—"}</span><span>herdr-core</span><span style="margin-left:auto">${illus("tail illustrative")}</span></div><div class="term">${tail.map((l) => (/PASS|ok /.test(l) ? `<span class="ok">${esc(l)}</span>` : /^\$|^Edit|^Bash|^Read/.test(l) ? `<span class="p">${esc(l)}</span>` : esc(l))).join("\n")}\n<span class="cur"></span></div></div>`;
  }
  function planCtx(id) {
    const t = T(id), p = D.PLANS.find((x) => x.id === t.plan);
    if (!p) return `<div class="empty" style="padding:16px"><p>Not part of a plan.</p></div>`;
    return `<div style="padding:10px 14px"><div style="font-size:12.5px;margin-bottom:6px"><b>Plan ${p.id}</b> <span class="dim">· ${esc(p.title)}</span></div>${window.PLANGRAPH(p, id, 340)}</div>`;
  }

  SCREENS.now = {
    desk(S) {
      const sel = S.sel;
      const dtab = (k, cur, label, key) => `<button class="dt" data-act="set" data-k="${key}" data-v="${k}" aria-selected="${cur === k}"><span class="grip"></span>${label}</button>`;
      const tools = `<span class="dtools"><button title="Split" aria-label="Split">${ic("split")}</button><button title="Zoom" aria-label="Zoom">${ic("zoom")}</button></span>`;
      const inspTabs = [["story", "Story"], ["evidence", "Evidence"], ["session", "Session"]];
      const insp = S.nowTab === "story" ? storyHtml(S, sel)
        : S.nowTab === "session" ? `<div class="insp"><p class="dim">Structured transcript with the raw terminal one tap away.</p><button class="btn" data-act="open-session" data-id="${sel}">${ic("chat")}Open ${sel} session</button></div>`
        : `<div class="insp"><div class="h3" style="margin-top:0">Evidence for ${sel}</div><ul class="steps">${["gate receipts · json", "checkpoint 3 · resume", "implementation report · md"].map((x) => `<li>${mk("na")}<span>${x}</span></li>`).join("")}</ul>${illus("artifact list illustrative")}</div>`;
      return `${connBanner()}<div class="cols stalefx" style="grid-template-columns:minmax(0,600px) minmax(0,1fr);grid-template-rows:minmax(0,1fr) 248px">
        <div class="dock" style="border-right:1px solid var(--rule)"><div class="dockbar" role="tablist">${dtab("list", "list", "Who acts next", "x")}<button class="dt" data-act="toast" data-msg="Goals is a filter: g toggles it">Goals</button>${tools}</div><div class="dockbody" data-scroll="now-list">${groupsHtml(S)}</div></div>
        <div class="dock"><div class="dockbar" role="tablist">${inspTabs.map(([k, l]) => dtab(k, S.nowTab, l, "nowTab")).join("")}${tools}</div><div class="dockbody" data-scroll="now-insp">${S.data === "loading" ? `<div class="insp" style="display:grid;gap:10px"><div class="skel" style="width:30%"></div><div class="skel" style="width:60%;height:18px"></div><div class="skel" style="height:60px"></div></div>` : insp}</div></div>
        <div class="dock" style="border-right:1px solid var(--rule)"><div class="dockbar">${dtab("since", S.lowTab, `Since your last visit · ${D.LAST_VISIT}`, "lowTab")}${dtab("held", S.lowTab, "Parked 32", "lowTab")}${tools}</div><div class="dockbody" data-scroll="now-since">${S.lowTab === "since" ? `<div class="feed">${feed(S)}</div>` : `<div class="feed">${D.HOLDS.map((h) => `<div class="ev"><span class="t">${h[3]}</span>${mk("na")}<span class="i">${h[0]}</span><span class="x">${esc(h[2])} <span class="dim">· ${h[1]}</span></span></div>`).join("")}</div>`}</div></div>
        <div class="dock"><div class="dockbar">${dtab("live", S.ctxTab, "Live", "ctxTab")}${dtab("plan", S.ctxTab, "Plan graph", "ctxTab")}${tools}</div><div class="dockbody" data-scroll="now-ctx">${S.ctxTab === "live" ? liveTail(sel) : planCtx(sel)}</div></div>
      </div>`;
    },
    phone(S) {
      let h = `<button class="pchip-row" style="width:100%" data-act="push" data-page="changes"><span class="chip">▲ 12 since ${D.LAST_VISIT}</span><span class="chip you">3 need you</span><span class="chip">5 moving</span><span class="chip">23 queued</span></button>`;
      if (S.data === "loading") return { title: "Now", large: true, body: h + `<div style="padding:12px 18px;display:grid;gap:14px">${[1, 2, 3, 4, 5, 6].map(() => `<div style="display:grid;gap:6px"><div class="skel" style="width:70%;height:14px"></div><div class="skel" style="width:40%"></div></div>`).join("")}</div>` };
      D.GROUPS.forEach((g) => {
        const n = g.key === "queued" ? 23 : g.key === "parked" ? 32 : g.ids.length + (g.extra ? g.extra.length : 0);
        const col = { you: "var(--you)", corral: "var(--corral)", worker: "var(--worker)", held: "var(--held)" }[g.key] || "var(--ink-2)";
        h += `<div class="ph-sec" style="color:${col}">${g.label}<span class="n">${n}</span></div>`;
        if (g.key === "you") {
          h += prow("T-868", { act: "push-q" }).replace('data-act="push-q"', 'data-act="push" data-page="question"');
          h += `<button class="prow" data-act="plan-open" data-id="T-885"><span class="actor a-you" style="width:10px;height:10px;border-radius:2px;transform:rotate(45deg) scale(.8);grid-row:1"></span><span class="l1"><span class="id">T-885</span><span class="tt">plan P2 · accept?</span></span><span></span><span class="l2"><span class="nw" style="color:var(--ok)">reviewed · 3 members</span><span class="r">10h</span></span></button>`;
        }
        g.ids.filter(() => g.key !== "you").slice(0, g.key === "queued" ? 4 : 9).forEach((id) => (h += prow(id)));
        if (g.more) h += `<button class="prow" style="min-height:48px;color:var(--ink-3);font-size:14px" data-act="toast" data-msg="+19 more queued, collapsed by reason">+ 19 more queued · after a dependency or capacity</button>`;
        if (g.parked) h += `<button class="prow" style="min-height:52px;font-size:14px;color:var(--ink-3)" data-act="toast" data-msg="Parked by you">${g.parked.split} · held by you</button>`;
      });
      return { title: "Now", large: true, body: h };
    },
    keys(e, S) {
      const ids = D.GROUPS.flatMap((g) => (g.key === "you" ? ["T-868"] : g.ids));
      const i = ids.indexOf(S.sel);
      if (e.key === "j" || e.key === "ArrowDown") { S.sel = ids[Math.min(ids.length - 1, i + 1)]; return; }
      if (e.key === "k" || e.key === "ArrowUp") { S.sel = ids[Math.max(0, i - 1)]; return; }
      if (e.key === "Enter") { if (S.sel === "T-868") S.screen = "question"; else { S.session = S.sel; S.screen = "session"; } return; }
      if (e.key === "Tab") { const o = ["story", "evidence", "session"]; S.nowTab = o[(o.indexOf(S.nowTab) + 1) % 3]; return; }
      return false;
    },
  };
  ACTIONS["open-session"] = (d) => { S.session = d.id; if (S.frame === "phone") { S.pstack.push({ page: "session" }); render(); } else nav("session"); };

  // phone task page
  SCREENS.task = {
    desk: (S) => SCREENS.now.desk(S),
    phone(S) {
      const id = S.sel, t = T(id);
      const body = `${storyHtml(S, id, true).replace('class="insp"', 'class="insp" style="padding:14px 18px"')}
        <div style="display:flex;gap:8px;padding:0 18px 14px"><button class="pbtn wide" data-act="open-session" data-id="${id}">${ic("chat")}Session</button><button class="pbtn ghost wide" data-act="ask-open" data-chip="${id}">Ask about this</button><button class="pbtn ghost" data-act="push" data-page="review">Review</button></div>
        <div class="ph-sec">Live<span class="r">${illus("illustrative")}</span></div>${liveTail(id).replace('style="padding:10px 12px"', 'style="padding:0 18px 10px"')}
        <div class="ph-sec">Plan ${t.plan || ""}</div><div style="padding:0 18px 20px">${t.plan ? window.PLANGRAPH(D.PLANS.find((p) => p.id === t.plan), id, 354, true) : `<span class="dim">Not part of a plan.</span>`}</div>`;
      return { title: `${id}`, backLabel: "Now", body };
    },
  };

  // ---------- Changes ----------
  SCREENS.changes = {
    desk(S) {
      return `${connBanner()}<div class="pagehead"><h1>Changes</h1><span class="sub">since this device's last visit ${D.LAST_VISIT} · 12 events · seen-cursor is per device: your phone read these at 21:30, the ▲ marks here stay</span><span class="acts"><div class="seg"><button aria-pressed="true">All</button><button>Needs you</button><button>Verdicts</button><button>Runs</button></div><button class="btn ghost" data-act="toast" data-msg="Seen-cursor moved to 22:12 for pawis-desktop only">Mark seen</button></span></div>
        <div class="pane stalefx" data-scroll="changes"><div class="feed" style="max-width:900px">${feed(S)}<div class="seenline">your last visit · ${D.LAST_VISIT} · seen-cursor for pawis-desktop</div>
        ${["18:12 T-885 review P2 approve · pi-sol", "17:40 T-854 checkpoint 2 captured", "16:40 T-852 held · external op started"].map((x) => { const [t, id, ...r] = x.split(" "); return `<div class="ev"><span class="t">${t}</span>${mk("na")}<span class="i">${id}</span><span class="x dim">${esc(r.join(" "))}</span></div>`; }).join("")}</div>
        <div class="illus" style="padding:8px 16px">Events after 18:40 are real snapshot changes; the three before the line and 21:59 are illustrative.</div></div>`;
    },
    phone(S) {
      return { title: "Changes", backLabel: S.ptab === "more" ? "More" : "Back", body: `<div class="pseg"><button aria-pressed="true">All</button><button>Needs you</button><button>Verdicts</button></div>${feed(S, 99, true)}<div class="seenline">last visit ${D.LAST_VISIT}</div>` };
    },
  };

  // ---------- phone More ----------
  SCREENS.more = {
    desk: (S) => SCREENS.now.desk(S),
    phone(S) {
      const it = (k, l, icon, sub) => `<button class="prow" style="grid-template-columns:28px 1fr 20px;min-height:56px" data-act="push" data-page="${k}">${ic(icon)}<span style="font-size:16px">${l}<span class="dim" style="display:block;font-size:13px">${sub}</span></span>${ic("chev")}</button>`;
      return { title: "More", large: true, body: `<div style="margin-top:6px;border-top:1px solid var(--rule)">${it("changes", "Changes", "changes", "12 since 18:40")}${it("review", "Review", "review", "3 candidates · 1 stale verdict")}${it("timeline", "Timeline", "timeline", "today · 1 stall")}${it("usage", "Usage", "usage", "OpenAI 78% of window")}${it("settings", "Settings", "settings", "Devices, Machines, notifications")}</div>` };
    },
  };
  window.SHEETPARTS = { situation, place, evidence, options, thread, history, feed, storyHtml, liveTail };
})();
