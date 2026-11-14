// Plans (graph, pipeline, acceptance, 64-member scale) and Timeline.
(() => {
  const { esc, ic, mk, rail, T, illus, money } = U;
  const NOWC = { you: "you", stale: "stale" };

  // Plan graph: left-to-right cards when every layer fits, vertical layers otherwise (Opus rule).
  window.PLANGRAPH = (p, selId, width, phone) => {
    const layers = p.layers || [p.members];
    const cw = 172, ch = 54, gx = 40, gy = 12;
    const fits = layers.length * cw + (layers.length - 1) * gx <= width;
    if (!fits) {
      return `<div class="vlayers">${layers.map((L, li) => `<div class="dim" style="font-size:10.5px;text-transform:uppercase;letter-spacing:.07em;margin:${li ? 10 : 0}px 0 4px">Layer ${li} · ${li ? "after layer " + (li - 1) : "can start now"}</div>
        ${L.map((id) => { const t = T(id); return `<button class="row" style="grid-template-columns:12px 46px minmax(0,1fr) 56px;height:${phone ? 40 : 28}px;padding:0 6px;${id === selId ? "background:var(--sel)" : ""}" data-act="${t.actor === "you" ? "goto" : "open-task"}" data-to="question" data-id="${id}"><span class="actor a-${t.actor}"></span><span class="id">${id}</span><span class="tt">${esc(t.title)}</span>${rail(t.rail)}</button>`; }).join("")}`).join("")}</div>`;
    }
    const pos = {}; let h = 0;
    layers.forEach((L, li) => L.forEach((id, mi) => { pos[id] = [li * (cw + gx), 18 + mi * (ch + gy)]; h = Math.max(h, 18 + (mi + 1) * (ch + gy)); }));
    let edges = "", nodes = "";
    layers.flat().forEach((id) => {
      const t = T(id);
      (t.needs || []).forEach(([dep]) => {
        if (!pos[dep]) return;
        const [x1, y1] = pos[dep], [x2, y2] = pos[id];
        const hot = t.actor === "you";
        edges += `<path class="edge ${hot ? "" : ""}" d="M${x1 + cw} ${y1 + ch / 2} C${x1 + cw + gx / 2} ${y1 + ch / 2}, ${x2 - gx / 2} ${y2 + ch / 2}, ${x2} ${y2 + ch / 2}"/>`;
      });
    });
    layers.forEach((L, li) => (nodes += `<text class="layer-h" x="${li * (cw + gx)}" y="10">Layer ${li}</text>`));
    layers.flat().forEach((id) => {
      const t = T(id), [x, y] = pos[id];
      const cls = (id === selId ? " sel" : "") + (t.actor === "you" ? " you" : "");
      const nowc = t.actor === "you" ? "you" : /stale/.test(t.now) ? "stale" : "";
      nodes += `<g class="gnode${cls}" data-act="${t.actor === "you" ? "goto" : "plan-member"}" data-to="question" data-id="${id}" transform="translate(${x},${y})" role="button" aria-label="${id} ${esc(t.title)}">
        <rect class="b" width="${cw}" height="${ch}" rx="6"/>
        <text class="gid" x="10" y="17">${id}</text>
        ${t.rail.map((k, i) => `<use href="#mk-${k}" x="${cw - 64 + i * 14}" y="7" width="11" height="11" class="mk ${k}${i === 0 ? " w" : ""}" style="color:${k === "done" ? "var(--ink-2)" : k === "active" ? (i === 0 ? "var(--worker)" : "var(--corral)") : k === "stale" ? "var(--held)" : k === "fail" ? "var(--fail)" : "var(--ink-4)"}"/>`).join("")}
        <text class="gt" x="10" y="33">${esc(t.title.length > 21 ? t.title.slice(0, 20) + "…" : t.title)}</text>
        <text class="gn ${nowc}" x="10" y="47">${esc(t.now)}</text></g>`;
    });
    const W = layers.length * cw + (layers.length - 1) * gx;
    return `<svg class="graph" viewBox="0 0 ${W} ${h}" style="max-width:${W}px" role="img" aria-label="Plan ${p.id} dependency graph">${edges}${nodes}</svg>`;
  };

  const plan = (id) => D.PLANS.find((p) => p.id === id) || D.PLANS[1];
  const landed = (p) => (p.members || []).filter((m) => T(m).rail[3] === "done").length;
  const memberBar = (p) => `<span class="sq">${(p.members || []).map((m) => { const t = T(m); return `<i class="${t.actor === "you" ? "you" : t.rail[3] === "done" ? "landed" : t.actor === "corral" || t.actor === "worker" ? "active" : t.actor === "held" ? "held" : ""}"></i>`; }).join("")}</span>`;

  function planList(S) {
    const sec = (l, n, c = "") => `<div class="grp ${c}"><span class="gl">${l}</span><span class="gn">${n}</span></div>`;
    const item = (p, extra) => `<button class="row" style="grid-template-columns:14px 54px minmax(0,1fr) auto;height:auto;min-height:40px;padding:6px 14px 6px 16px" data-act="set" data-k="plan" data-v="${p.id}" aria-selected="${S.plan === p.id && !S.planScale}">
      <span class="actor a-${p.state === "awaiting" ? "you" : p.state === "executing" ? "corral" : "queued"}"></span><span class="id">${p.id}${p.rev ? " " + p.rev : ""}</span>
      <span style="min-width:0"><span class="tt" style="display:block">${esc(p.title)}</span><span class="dim" style="font-size:11.5px">${extra}</span></span><span class="mono dim" style="font-size:11px">${p.state === "executing" ? landed(p) + "/" + p.members.length : ""}</span></button>`;
    let h = sec("Awaiting your acceptance", 1, "g-you") + item(D.PLANS[0], `reviewed · approve P2 · 3 members`);
    h += sec("Executing", 4, "g-corral") + D.PLANS.filter((p) => p.state === "executing").map((p) => item(p, `${memberBar(p)} &nbsp;criteria <span class="c-held">${p.mapped}/${p.scenarios} mapped</span>`)).join("");
    h += sec("Approved · not started", 1, "g-queued") + item(D.PLANS[5], D.PLANS[5].note);
    h += sec("Scale test", 1, "g-parked") + `<button class="row" style="grid-template-columns:14px 54px minmax(0,1fr);height:40px" data-act="set" data-k="planScale" data-v="true" aria-selected="${S.planScale}"><span class="actor a-queued"></span><span class="id">S-900</span><span class="tt">64-member tracker ${illus("synthetic")}</span></button>`;
    return `<div class="list">${h}</div>`;
  }

  function pipeline(stages) {
    return `<div class="pipe">${stages.map(([k, l], i) => `${i ? '<span class="pl"></span>' : ""}<span class="ps ${k}">${mk(k === "on" ? "active" : k === "done" ? "done" : "next")}${l}</span>`).join("")}</div>`;
  }

  function awaiting(S, phone) {
    const p = D.PLANS[0];
    const off = ["offline", "down"].includes(S.health);
    const doc = `<div class="plan-doc" style="padding:${phone ? "14px 18px" : "16px 24px"}">
      ${pipeline([["on", "Design · awaiting you"], ["", "Members 0/3"], ["", "Aggregate verify"], ["", "Final accept"]])}
      <h3>Problem</h3><p>${esc(p.problem)}</p><h3>Goal</h3><p>${esc(p.goal)}</p>
      <h3>Changes</h3><ul>${p.changes.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>
      <h3>Keeps</h3><ul>${p.retained.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>
      <h3>Not doing</h3><ul>${p.nonGoals.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>
      <h3>Proposed members · published on accept</h3>
      <div style="display:grid;gap:4px">${p.members.map(([m, deps], i) => `<div style="display:grid;grid-template-columns:22px 1fr auto;gap:8px;align-items:center;font-size:13px">${mk("next")}<span class="mono">${m}</span><span class="dim" style="font-size:12px">${deps.length ? "after " + deps[0] : "can start now"}</span></div>`).join("")}</div>
      <h3>Reviewer verdicts · bound to plan revision</h3>
      <div style="display:grid;gap:6px">${p.reviews.map(([rev, k, v, who, at, note]) => `<div style="display:grid;grid-template-columns:28px 20px 70px minmax(0,1fr);gap:8px;align-items:baseline;font-size:12.5px;${rev === "P1" ? "opacity:.75" : ""}"><span class="mono">${rev}</span>${mk(rev === "P1" ? "stale" : k)}<span>${v}${rev === "P1" ? '<span class="dim"> · superseded</span>' : ""}</span><span><span class="dim">${who} ${at} ·</span> ${esc(note)}</span></div>`).join("")}</div>
      <h3>What accepting does</h3><p>Publishes 3 member tasks to <b>main</b> delivery and creates tracker T-893. Autopilot may admit the first member; gates, independent review and landing checks stay unchanged.</p>
      <div class="dim" style="font-size:12px;margin-top:10px">Document <span class="mono">${esc(p.artifact)}</span></div></div>`;
    const bar = off ? `<button class="btn" disabled>Accept · publish 3 members</button><span class="attr" style="color:var(--fail)">Refused offline: acceptance is revision-checked when it applies.</span>`
      : `<button class="btn" data-act="confirm" data-kind="accept-plan">Accept · publish 3 members</button><button class="btn ghost" data-act="toast" data-msg="Request changes: comment box opened">Request changes</button><button class="btn quiet" data-act="toast" data-msg="Diff P1 → P2 opened">Diff P1 → P2</button><button class="btn quiet" data-act="toast" data-msg="Opened plan document">Open document</button><span class="attr">Runs <span class="mono">corral plan accept T-885 --rev P2</span> after a fresh confirm<br><span class="mono">actor=owner · via=app:pawis-desktop</span></span>`;
    if (phone) return { body: `<div class="pband"><div class="meta" style="display:flex;gap:6px;flex-wrap:wrap"><span class="chip you">needs you</span><span class="chip ok">reviewed · approve P2</span><span class="mono dim" style="font-size:12px">T-885 P2</span></div><div style="font-size:20px;font-weight:700;line-height:1.25;margin-top:8px">${esc(p.title)}</div></div>${doc}`,
      decide: off ? `<button class="pbtn wide" disabled>Accept · refused offline</button>` : `<button class="pbtn ghost" data-act="toast" data-msg="Request changes">Changes</button><button class="pbtn wide" data-act="confirm" data-kind="accept-plan">Accept · publish 3</button>` };
    return `<div style="display:flex;flex-direction:column;min-height:100%"><div class="qhead"><div class="meta"><span class="chip you">needs you</span><span class="chip ok">reviewed · approve P2</span><span class="mono">T-885 P2</span><span>replay of 12:10–19:12 ${illus("timing replay")}</span></div><h1>${esc(p.title)}</h1></div>${doc}<div style="flex:1"></div><div class="decide">${bar}</div></div>`;
  }

  function executing(S, p, phone) {
    const sel = p.members.includes(S.planMember) ? S.planMember : p.members[0];
    const L = landed(p);
    const head = `<div class="${phone ? "pband" : "qhead"}"><div class="meta" style="display:flex;gap:6px;flex-wrap:wrap;align-items:center"><span class="chip corral">executing</span><span class="mono">${p.id}</span><span class="dim">accepted ${p.accepted} · delivers to main</span>${phone ? "" : `<button class="btn ghost" style="margin-left:auto;height:26px" data-act="ask-open" data-chip="${p.id}">${ic("chat")}Ask about this</button>`}</div><${phone ? "div style=\"font-size:19px;font-weight:700;line-height:1.25;margin-top:8px\"" : "h1"}>${esc(p.title)}</${phone ? "div" : "h1"}></div>`;
    const body = `<div style="padding:${phone ? "14px 18px" : "16px 24px"}">
      ${pipeline([["done", "Design"], ["on", `Members ${L}/${p.members.length} landed`], ["", "Aggregate verify"], ["", "Final accept"]])}
      <div class="warnbar">${mk("stale")}<span>${p.scenarios} of ${p.scenarios} acceptance scenarios are not mapped to any member · final verify has no owner</span></div>
      <div class="h3" style="margin-top:0">Members · live state</div>
      ${window.PLANGRAPH(p, sel, phone ? 354 : 760, phone)}
      <div class="h3">Blocking final acceptance <span class="r">${p.blockers.length}</span></div>
      <ul class="steps">${p.blockers.map((b) => `<li>${mk(/your answer/.test(b) ? "active" : "na")}<span class="${/your answer/.test(b) ? "c-you" : ""}">${esc(b)}</span></li>`).join("")}</ul></div>`;
    if (phone) return { body: head + body };
    return `<div class="cols" style="grid-template-columns:minmax(0,1fr) 340px;height:100%"><div class="pane" data-scroll="plan-main">${head}${body}</div><div class="pane side-col" data-scroll="plan-side" style="padding:0">${window.SHEETPARTS.storyHtml(S, sel)}</div></div>`;
  }

  function scale(S, phone) {
    const L = D.SCALE;
    const total = L.flat().length, landedN = L.flat().filter((m) => m.st === "landed").length;
    const rows = L.map((layer, li) => {
      const act = layer.filter((m) => m.st !== "landed" && m.st !== "waiting");
      const land = layer.filter((m) => m.st === "landed").length;
      const wait = layer.filter((m) => m.st === "waiting").length;
      const shown = act.slice(0, phone ? 3 : 4);
      return `<div style="border-bottom:1px solid var(--rule);padding:8px 0">
        <div style="display:grid;grid-template-columns:${phone ? "70px 1fr" : "110px auto 1fr"};gap:10px;align-items:center;font-size:12.5px"><b>Layer ${li}</b><span class="sq">${layer.map((m) => `<i class="${m.st}"></i>`).join("")}</span><span class="dim" style="${phone ? "grid-column:1/3" : ""}">${land ? `${land} landed` : ""}${land && (act.length || wait) ? " · " : ""}${act.length ? `${act.length} moving or need you` : ""}${act.length && wait ? " · " : ""}${wait ? `${wait} waiting on layer ${li - 1}` : ""}</span></div>
        ${shown.map((m) => `<div style="display:grid;grid-template-columns:14px 52px minmax(0,1fr) auto;gap:8px;align-items:center;font-size:12.5px;padding:3px 0 0 ${phone ? 0 : 110}px"><span class="actor a-${m.st === "you" ? "you" : m.st === "held" ? "held" : "corral"}" style="width:9px;height:9px;border-radius:50%"></span><span class="mono dim">${m.id}</span><span>${m.name}</span><span class="${m.st === "you" ? "c-you" : m.st === "held" ? "c-held" : "dim"}">${m.st === "you" ? "needs you" : m.st === "held" ? "held · budget" : "gating"}</span></div>`).join("")}
        ${act.length > shown.length ? `<div class="dim" style="font-size:12px;padding-left:${phone ? 0 : 110}px">+${act.length - shown.length} more in this layer</div>` : ""}</div>`;
    }).join("");
    const inner = `<div style="padding:${phone ? "14px 18px" : "16px 24px"}">${pipeline([["done", "Design"], ["on", `Members ${landedN}/${total} landed`], ["", "Aggregate verify"], ["", "Final accept"]])}
      <div class="dim" style="font-size:12.5px;margin-bottom:8px">Landed members collapse into the layer strip; only members that move or need you get a line. Each square is one member, left to right in id order.</div>${rows}
      <div class="legend" style="padding:10px 0 0"><span><i style="background:var(--ink-3)"></i>landed</span><span><i style="background:var(--corral)"></i>moving</span><span><i style="background:var(--you)"></i>needs you</span><span><i style="background:var(--held)"></i>held</span><span><i style="background:var(--rule-2)"></i>waiting</span></div></div>`;
    const head = `<div class="${phone ? "pband" : "qhead"}"><div class="meta" style="display:flex;gap:6px;align-items:center"><span class="chip corral">executing</span><span class="mono">S-900</span>${illus("synthetic scale test · 64 members · 6 layers")}</div><div style="font-size:${phone ? 19 : 19}px;font-weight:700;margin-top:6px">Store tracker: 64 members</div></div>`;
    return phone ? { body: head + inner } : head + inner;
  }

  SCREENS.plans = {
    desk(S) {
      const p = plan(S.plan);
      const right = S.planScale ? scale(S) : p.state === "awaiting" ? awaiting(S) : p.state === "approved" ? `<div class="empty" style="padding:40px"><h3>T-385 P5 · approved, no tracker published</h3><p>4 approvals. Publishing members is a separate owner action.</p></div>` : executing(S, p);
      return `${connBanner()}<div class="cols stalefx" style="grid-template-columns:380px minmax(0,1fr)"><div class="pane" data-scroll="plans-l">${planList(S)}</div><div class="pane" data-scroll="plans-r">${right}</div></div>`;
    },
    phone(S) {
      const it = (p, sub) => `<button class="prow" data-act="phone-plan" data-id="${p.id}"><span class="actor a-${p.state === "awaiting" ? "you" : p.state === "executing" ? "corral" : "queued"}" style="width:10px;height:10px;border-radius:50%;grid-row:1"></span><span class="l1"><span class="id">${p.id}</span><span class="tt">${esc(p.title)}</span></span><span class="mono dim" style="font-size:12px">${p.state === "executing" ? landed(p) + "/" + p.members.length : ""}</span><span class="l2">${sub}</span></button>`;
      const body = `<div class="ph-sec" style="color:var(--you)">Awaiting your acceptance<span class="n">1</span></div>${it(D.PLANS[0], "reviewed · approve P2 · 3 members")}
        <div class="ph-sec" style="color:var(--corral)">Executing<span class="n">4</span></div>${D.PLANS.filter((p) => p.state === "executing").map((p) => it(p, `${memberBar(p)}<span class="c-held" style="font-size:12.5px">criteria ${p.mapped}/${p.scenarios} mapped</span>`)).join("")}
        <div class="ph-sec">Approved · not started<span class="n">1</span></div>${it(D.PLANS[5], D.PLANS[5].note)}
        <div class="ph-sec">Scale test<span class="r">${illus("synthetic")}</span></div><button class="prow" data-act="phone-plan" data-id="S-900"><span class="actor a-queued" style="width:10px;height:10px;border-radius:50%;grid-row:1"></span><span class="l1"><span class="id">S-900</span><span class="tt">64-member tracker</span></span><span></span><span class="l2">6 layers · 64 members</span></button>`;
      return { title: "Plans", large: true, body };
    },
    keys(e, S) {
      const ids = ["T-885", "T-870", "T-893", "T-898", "T-874", "T-385"];
      const i = ids.indexOf(S.plan);
      if (e.key === "j" || e.key === "ArrowDown") { S.planScale = false; S.plan = ids[Math.min(5, i + 1)]; return; }
      if (e.key === "k" || e.key === "ArrowUp") { S.planScale = false; S.plan = ids[Math.max(0, i - 1)]; return; }
      if (e.key === "x") { S.planScale = !S.planScale; return; }
      if (e.key === "J" || e.key === "K") { const p = plan(S.plan); if (p.members) { const m = p.members.indexOf(S.planMember); S.planMember = p.members[(m + (e.key === "J" ? 1 : p.members.length - 1)) % p.members.length]; } return; }
      return false;
    },
  };
  SCREENS.plan = {
    desk: (S) => SCREENS.plans.desk(S),
    phone(S) {
      if (S.planScale) { const r = scale(S, true); return { title: "S-900", backLabel: "Plans", body: r.body }; }
      const p = plan(S.plan);
      if (p.state === "awaiting") { const r = awaiting(S, true); return { title: "Plan review", backLabel: "Plans", body: r.body, decide: r.decide }; }
      if (p.state === "approved") return { title: p.id, backLabel: "Plans", body: `<div class="empty"><h3>Approved · not started</h3><p>${p.note}</p></div>` };
      return { title: p.id, backLabel: "Plans", body: executing(S, p, true).body, right: `<button data-act="ask-open" data-chip="${p.id}" aria-label="Ask the assistant about this plan">${ic("chat")}</button>` };
    },
  };
  Object.assign(ACTIONS, {
    "phone-plan": (d) => { S.planScale = d.id === "S-900"; if (!S.planScale) S.plan = d.id; S.pstack.push({ page: "plan" }); render(); },
    "plan-member": (d) => { S.planMember = d.id; if (S.frame === "phone") { S.sel = d.id; S.pstack.push({ page: "task" }); } render(); },
  });

  // ---------- Timeline ----------
  const mins = (hhmm) => { const [h, m] = hhmm.split(":").map(Number); return h * 60 + m; };
  function timeline(S, W, phone) {
    const { start, end, lanes, owner, pm, lastVisit } = D.TL;
    const labelW = phone ? 0 : 190, laneH = phone ? 46 : 34, top = 26;
    const plotW = W - labelW - 10;
    const X = (m) => labelW + ((m - start) / (end - start)) * plotW;
    let g = "";
    for (let h = 9; h <= 22; h++) { const x = X(h * 60); g += `<line class="grid" x1="${x}" x2="${x}" y1="${top - 6}" y2="9999"/><text class="ax" x="${x}" y="14" text-anchor="middle">${phone && h % 2 ? "" : String(h).padStart(2, "0")}</text>`; }
    let y = top;
    const laneLabel = (id, t) => phone ? `<text class="lanel" x="0" y="${y + 11}">${id}</text><text class="lanet" x="${id.length * 7 + 8}" y="${y + 11}">${esc(t)}</text>` : `<text class="lanel" x="0" y="${y + laneH / 2 + 4}">${id}</text><text class="lanet" x="52" y="${y + laneH / 2 + 4}">${esc(t)}</text>`;
    const by = (h = 12) => (phone ? y + 18 : y + (laneH - h) / 2);
    // owner lane
    g += laneLabel("You", phone ? "owner actions" : "owner actions");
    owner.forEach(([t, what]) => { const x = X(mins(t)); g += `<g><circle cx="${x}" cy="${by(10) + 5}" r="4.5" fill="var(--you)"><title>${t} ${esc(what)}</title></circle></g>`; });
    y += laneH;
    g += laneLabel("PM", "supervisor");
    pm.forEach(([a, b, st]) => { g += `<rect x="${X(mins(a))}" y="${by(6) + 3}" width="${X(mins(b)) - X(mins(a))}" height="6" rx="2" fill="${st ? "var(--held)" : "var(--ink-4)"}"/>`; if (st) g += `<text class="ax" x="${X(mins(a))}" y="${by() + 22}">relaunching</text>`; });
    y += laneH + 6;
    lanes.forEach((L) => {
      g += laneLabel(L.id, L.label);
      L.segs.forEach(([k, a, b, note]) => {
        const x1 = X(mins(a)), x2 = X(mins(b));
        g += `<rect class="seg-${k}" x="${x1}" y="${by()}" width="${Math.max(2, x2 - x1 - 1)}" height="12" rx="2"><title>${L.id} ${k} ${a}–${b}${note ? " · " + note : ""}</title></rect>`;
        if (k === "stall") g += `<text class="stall-l" x="${phone ? x1 - 60 : x1 + 4}" y="${by() + (phone ? 25 : -3)}">${phone ? `stall ${a}–${b} · 2h48` : `stall ${a}–${b} · nobody moved`}</text>`;
        if (k === "ask" && !phone) g += `<text class="ask-l" x="${x1 + 4}" y="${by() - 3}">waiting on you · 7h02</text>`;
      });
      if (L.ask) { const x1 = X(mins(L.ask[0])), x2 = X(mins(L.ask[1])); g += `<rect class="seg-ask" x="${x1}" y="${by() - 6}" width="${x2 - x1}" height="4" rx="1"><title>${L.ask[2]}</title></rect>`; if (!phone) g += `<text class="ask-l" x="${x1 + 4}" y="${by() - 9}">needs you 3h</text>`; }
      y += laneH;
    });
    g += `<line class="lv" x1="${X(lastVisit)}" x2="${X(lastVisit)}" y1="${top - 4}" y2="${y}"/><text class="ax" x="${X(lastVisit) - 4}" y="${y + 12}" text-anchor="end">your last visit 18:40</text>`;
    g += `<line class="now" x1="${X(end)}" x2="${X(end)}" y1="${top - 4}" y2="${y}"/><text class="ax" x="${X(end) - 2}" y="${y + 12}" text-anchor="end" style="fill:var(--ink)">now</text>`;
    return `<svg viewBox="0 0 ${W} ${y + 18}" role="img" aria-label="Run timeline 09:00 to 22:12">${g}</svg>`;
  }
  const legend = `<div class="legend"><span><i style="background:var(--worker)"></i>run</span><span><i style="background:var(--corral)"></i>gate</span><span><i style="background:color-mix(in oklch, var(--corral) 55%, var(--ink-3))"></i>review</span><span><i style="background:var(--ok)"></i>land</span><span><svg width="18" height="8"><rect width="18" height="8" fill="url(#hatch-ask)"/></svg>waiting on you</span><span><svg width="18" height="8"><rect width="18" height="8" fill="url(#hatch)"/></svg>idle</span><span><svg width="18" height="8"><rect width="18" height="8" fill="url(#hatch-stale)"/></svg>stale verdict</span><span><svg width="18" height="8"><rect width="18" height="8" fill="url(#hatch-stall)"/></svg>stall</span></div>`;
  const stalls = `<div class="h3" style="margin:6px 18px 6px">Gaps worth a look today</div><div style="padding:0 18px 18px;display:grid;gap:6px;font-size:12.5px;max-width:880px">
    <div style="display:grid;grid-template-columns:96px 54px 1fr;gap:8px"><span class="mono c-fail">16:40–19:28</span><span class="mono dim">T-852</span><span>held · external op; no sync tick moved it for 2h48. Today the daemon would have re-checked on its own loop.</span></div>
    <div style="display:grid;grid-template-columns:96px 54px 1fr;gap:8px"><span class="mono c-you">12:10–19:12</span><span class="mono dim">T-885</span><span>plan P2 waited 7h02 for your acceptance; a push would have reached the phone at 12:10.</span></div>
    <div style="display:grid;grid-template-columns:96px 54px 1fr;gap:8px"><span class="mono c-you">19:12–now</span><span class="mono dim">T-868</span><span>question open 3h; the worker kept producing until 21:40, then idled.</span></div>
    <div class="illus">Segment boundaries are illustrative, reconstructed around real start times and change events. Real records show 4 stalls of 4–8.6 h ended only by the owner asking (docs/records/pm-runtime-redesign.md).</div></div>`;
  SCREENS.timeline = {
    desk(S) {
      return `${connBanner()}<div class="pagehead"><h1>Timeline</h1><span class="sub">who worked when · run, gate, review, land · idle and stalls</span><span class="acts"><div class="seg"><button aria-pressed="true">Today</button><button>7 days</button></div><div class="seg"><button aria-pressed="true">By task</button><button>By agent</button></div></span></div>
        <div class="pane stalefx" data-scroll="tl">${legend}<div class="tl">${timeline(S, 1190, false)}</div><div class="cols" style="grid-template-columns:minmax(0,1fr) 440px;align-items:start"><div>${stalls}</div><div style="padding:0 18px 18px">${burnup(404, 150)}</div></div></div>`;
    },
    phone(S) {
      return { title: "Timeline", backLabel: "More", body: `<div class="pseg"><button aria-pressed="true">Today</button><button>7 days</button></div>${legend.replace('class="legend"', 'class="legend" style="padding:4px 18px 8px;font-size:12px"')}<div class="tl" style="padding:0 14px 10px">${timeline(S, 362, true)}</div>${stalls.replace(/grid-template-columns:96px 54px 1fr/g, "grid-template-columns:1fr").replace("margin:6px 18px 6px", "margin:6px 18px")}<div style="padding:0 18px 24px">${burnup(354, 140)}</div>` };
    },
  };

  // R23 goal burn-up: landed vs scope per day, one line pair per goal. Illustrative numbers.
  function burnup(w, h) {
    const scope = [41, 41, 44, 44, 47, 47, 50, 53, 53, 53, 56, 56, 56, 57], done = [18, 19, 19, 22, 24, 24, 27, 29, 31, 31, 33, 35, 36, 37];
    const pad = 26, X = (i) => pad + (i / 13) * (w - pad - 6), Y = (v) => 8 + (1 - v / 60) * (h - 30);
    const line = (a, cls) => `<polyline fill="none" ${cls} points="${a.map((v, i) => `${X(i)},${Y(v)}`).join(" ")}"/>`;
    return `<div class="h3" style="margin:6px 0 6px">Goal burn-up · Co-work and PM goals <span class="r">14 days</span></div>
      <svg class="band-chart" viewBox="0 0 ${w} ${h}" role="img" aria-label="Burn-up: 37 of 57 landed">
      ${[0, 20, 40, 60].map((v) => `<line x1="${pad}" x2="${w - 6}" y1="${Y(v)}" y2="${Y(v)}" stroke="var(--rule)"/><text class="zl" x="0" y="${Y(v) + 3}">${v}</text>`).join("")}
      ${line(scope, 'stroke="var(--ink-3)" stroke-dasharray="4 3" stroke-width="1.4"')}${line(done, 'stroke="var(--ink)" stroke-width="1.8"')}
      <text class="ax" x="${X(13)}" y="${Y(57) - 5}" text-anchor="end">scope 57</text><text class="ax" x="${X(13)}" y="${Y(37) + 14}" text-anchor="end">landed 37</text>
      <text class="ax" x="${pad}" y="${h - 4}">10-26</text><text class="ax" x="${w - 6}" y="${h - 4}" text-anchor="end">today</text></svg>
      <div class="illus">burn-up numbers illustrative</div>`;
  }
})();
