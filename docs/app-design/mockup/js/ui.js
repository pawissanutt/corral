// Screen and action registries, filled by screens-*.js and app.js.
window.SCREENS = {};
window.ACTIONS = {};
// Shared rendering helpers: marks, icons, rows, health line. Plain functions returning HTML strings.
window.U = (() => {
  const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const T = (id) => D.TASKS[id] || { title: id, rail: ["next", "next", "next", "next"], actor: "queued", now: "" };

  const SPRITE = `<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs>
  <symbol id="mk-done" viewBox="0 0 12 12"><path d="M2.4 6.4l2.4 2.4 4.8-5.2" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></symbol>
  <symbol id="mk-active" viewBox="0 0 12 12"><circle cx="6" cy="6" r="4.6" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M6 1.4a4.6 4.6 0 0 0 0 9.2z" fill="currentColor"/></symbol>
  <symbol id="mk-fail" viewBox="0 0 12 12"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></symbol>
  <symbol id="mk-stale" viewBox="0 0 12 12"><circle cx="6" cy="6" r="4.6" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.1 1.5"/></symbol>
  <symbol id="mk-next" viewBox="0 0 12 12"><circle cx="6" cy="6" r="4.2" fill="none" stroke="currentColor" stroke-width="1.2"/></symbol>
  <symbol id="mk-na" viewBox="0 0 12 12"><circle cx="6" cy="6" r="1.3" fill="currentColor"/></symbol>
  <symbol id="i-inbox" viewBox="0 0 20 20"><path d="M3 12l2-7.5h10l2 7.5M3 12v4.5h14V12M3 12h4.2l1 2h3.6l1-2H17"/></symbol>
  <symbol id="i-now" viewBox="0 0 20 20"><path d="M8 5.5h9M8 10h9M8 14.5h9"/><circle cx="4" cy="5.5" r="1.4"/><circle cx="4" cy="10" r="1.4"/><circle cx="4" cy="14.5" r="1.4"/></symbol>
  <symbol id="i-changes" viewBox="0 0 20 20"><path d="M10 3.5l7 13H3z"/><path d="M10 8.5v3.5"/></symbol>
  <symbol id="i-plans" viewBox="0 0 20 20"><rect x="2.5" y="3" width="5" height="4" rx="1"/><rect x="2.5" y="13" width="5" height="4" rx="1"/><rect x="12.5" y="8" width="5" height="4" rx="1"/><path d="M7.5 5h2.5v10H7.5M10 10h2.5"/></symbol>
  <symbol id="i-agents" viewBox="0 0 20 20"><rect x="2.5" y="3.5" width="15" height="13" rx="2"/><path d="M6 8.5l2.2 2-2.2 2M10.5 12.5H14"/></symbol>
  <symbol id="i-review" viewBox="0 0 20 20"><rect x="2.5" y="3" width="15" height="14" rx="2"/><path d="M10 3v14M4.8 10h3M6.3 8.5v3M12.3 10h3"/></symbol>
  <symbol id="i-timeline" viewBox="0 0 20 20"><path d="M3 5h7M6 10h10M4.5 15h6"/><path d="M3 2.5v15" opacity=".5"/></symbol>
  <symbol id="i-usage" viewBox="0 0 20 20"><path d="M3 14.5a7 7 0 0 1 14 0"/><path d="M10 14.5l3.2-4.2"/><path d="M3 17h14" opacity=".5"/></symbol>
  <symbol id="i-settings" viewBox="0 0 20 20"><path d="M3 6h2.5M9.5 6H17M3 14h7.5M14.5 14H17"/><circle cx="7.5" cy="6" r="2"/><circle cx="12.5" cy="14" r="2"/></symbol>
  <symbol id="i-more" viewBox="0 0 20 20"><circle cx="4.5" cy="10" r="1.2"/><circle cx="10" cy="10" r="1.2"/><circle cx="15.5" cy="10" r="1.2"/></symbol>
  <symbol id="i-search" viewBox="0 0 20 20"><circle cx="9" cy="9" r="5.5"/><path d="M13 13l4 4"/></symbol>
  <symbol id="i-back" viewBox="0 0 20 20"><path d="M12.5 4l-6 6 6 6"/></symbol>
  <symbol id="i-chev" viewBox="0 0 20 20"><path d="M8 5l5 5-5 5"/></symbol>
  <symbol id="i-close" viewBox="0 0 20 20"><path d="M5 5l10 10M15 5L5 15"/></symbol>
  <symbol id="i-zoom" viewBox="0 0 20 20"><path d="M4 8V4h4M16 12v4h-4M4 4l5 5M16 16l-5-5"/></symbol>
  <symbol id="i-split" viewBox="0 0 20 20"><rect x="3" y="4" width="14" height="12" rx="1.5"/><path d="M10 4v12"/></symbol>
  <symbol id="i-term" viewBox="0 0 20 20"><rect x="2.5" y="3.5" width="15" height="13" rx="2"/><path d="M6 8.5l2.2 2-2.2 2M10.5 12.5H14"/></symbol>
  <symbol id="i-chat" viewBox="0 0 20 20"><path d="M4 4.5h12v8.5H9l-3.5 3v-3H4z"/><path d="M7 8h6M7 10.5h4"/></symbol>
  <symbol id="i-send" viewBox="0 0 20 20"><path d="M10 16V4M5 9l5-5 5 5"/></symbol>
  <symbol id="i-lock" viewBox="0 0 20 20"><rect x="4" y="9" width="12" height="8" rx="1.5"/><path d="M7 9V6.5a3 3 0 0 1 6 0V9"/></symbol>
  <symbol id="i-phone" viewBox="0 0 20 20"><rect x="5.5" y="2.5" width="9" height="15" rx="2"/><path d="M9 15h2"/></symbol>
  <symbol id="i-qr" viewBox="0 0 20 20"><rect x="3" y="3" width="5" height="5"/><rect x="12" y="3" width="5" height="5"/><rect x="3" y="12" width="5" height="5"/><path d="M12 12h2v2M17 12v5h-3M12 17h.5"/></symbol>
  <symbol id="i-signal" viewBox="0 0 20 14"><rect x="1" y="9" width="3" height="4" rx=".7" fill="currentColor" stroke="none"/><rect x="6" y="6" width="3" height="7" rx=".7" fill="currentColor" stroke="none"/><rect x="11" y="3" width="3" height="10" rx=".7" fill="currentColor" stroke="none"/><rect x="16" y="0" width="3" height="13" rx=".7" fill="currentColor" stroke="none"/></symbol>
  <symbol id="i-batt" viewBox="0 0 28 14"><rect x="1" y="1" width="23" height="12" rx="3.5" fill="none" stroke="currentColor" opacity=".45"/><rect x="3" y="3" width="16" height="8" rx="2" fill="currentColor" stroke="none"/><path d="M26 5v4" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></symbol>
  <symbol id="i-brand" viewBox="0 0 20 20"><circle cx="10" cy="10" r="7" fill="none" stroke-dasharray="3.2 2.3" stroke-width="2"/><circle cx="10" cy="10" r="2.2" fill="currentColor" stroke="none"/></symbol>
  <pattern id="hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="6" height="6" fill="transparent"/><path d="M0 0v6" stroke="var(--ink-4)" stroke-width="1.4" opacity=".55"/></pattern>
  <pattern id="hatch-stall" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="6" height="6" fill="var(--fail-bg)"/><path d="M0 0v6" stroke="var(--fail)" stroke-width="1.6"/></pattern>
  <pattern id="hatch-stale" width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="5" height="5" fill="var(--held-bg)"/><path d="M0 0v5" stroke="var(--held)" stroke-width="1.4"/></pattern>
  <pattern id="hatch-ask" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(-45)"><rect width="6" height="6" fill="var(--you-bg)"/><path d="M0 0v6" stroke="var(--you)" stroke-width="1.4"/></pattern>
</defs></svg>`;

  const ic = (name, cls = "") => `<svg class="ic ${cls}" aria-hidden="true"><use href="#i-${name}"/></svg>`;
  const LABEL = { done: "passed", active: "running", fail: "failed", stale: "stale (older candidate)", next: "not yet", na: "not applicable" };
  const STAGES = ["run", "gate", "review", "land"];
  const mk = (kind, worker = false) => `<svg class="mk ${kind}${worker ? " w" : ""}" role="img" aria-label="${LABEL[kind]}"><use href="#mk-${kind}"/></svg>`;
  const rail = (r) => `<span class="rail" role="img" aria-label="${r.map((k, i) => STAGES[i] + " " + LABEL[k]).join(", ")}">${r.map((k, i) => mk(k, i === 0)).join("")}</span>`;
  const money = (c) => (c == null ? "" : "$" + (c >= 10 ? c.toFixed(0) : c.toFixed(2)));
  const nowClass = (t) => (t.actor === "you" ? "you" : t.actor === "held" ? "held" : /stale/.test(t.now) ? "stale" : "");

  // Desktop one-line row: Δ · actor · id · title · rail · now · age · cost
  const row = (id, sel, opts = {}) => {
    const t = T(id);
    return `<button class="row r-${t.actor}" data-act="select" data-id="${id}" aria-selected="${sel === id}" role="option">
      <span>${t.changed ? '<span class="delta" title="changed since your last visit"></span>' : ""}</span>
      <span class="actor a-${t.actor}"></span>
      <span class="id">${id}</span>
      <span class="tt">${esc(t.title)}</span>
      ${rail(t.rail)}
      <span class="nw ${nowClass(t)}">${esc(t.now)}</span>
      <span class="ag">${t.age || ""}</span>
      <span class="co ${t.cost > 20 ? "hot" : ""}">${money(t.cost)}</span>
    </button>`;
  };
  // Phone two-line row: line 1 id · title · rail, line 2 now · age · cost
  const prow = (id, opts = {}) => {
    const t = T(id);
    return `<button class="prow r-${t.actor}" data-act="${opts.act || "open-task"}" data-id="${id}">
      <span class="actor a-${t.actor}" style="width:10px;height:10px;border-radius:50%;grid-row:1"></span>
      <span class="l1"><span class="id">${id}</span><span class="tt">${esc(t.title)}</span></span>
      ${rail(t.rail)}
      <span class="l2">${t.changed ? '<span class="delta"></span>' : ""}<span class="nw ${nowClass(t) ? "c-" + (nowClass(t) === "stale" ? "held" : nowClass(t)) : ""}">${esc(t.now)}</span><span class="r">${[t.age, money(t.cost)].filter(Boolean).join(" · ")}</span></span>
    </button>`;
  };

  // Health line. Contract strings verbatim: daemon healthy | degraded · <reason> | down; runtime herdr-core ok | unreachable; PM live | relaunching | absent.
  // A second runtime segment (herdr-tui) shows while any open run is on the legacy adapter (T-854 is, in this snapshot).
  const HEALTH = {
    healthy:  { d: ["healthy", ""], r: ["herdr-core ok", ""], t: ["herdr-tui ok", ""], p: ["live", ""], reach: "direct · loopback", preach: "direct · tailscale" },
    degraded: { d: ["degraded · sync stalled 4m", "warn"], r: ["herdr-core ok", ""], t: ["herdr-tui ok", ""], p: ["relaunching", "warn"], reach: "direct · loopback", preach: "direct · tailscale" },
    down:     { d: ["down", "bad"], r: ["herdr-core unreachable", "bad"], t: ["herdr-tui unreachable", "bad"], p: ["absent", "bad"], reach: "direct · loopback", preach: "direct · tailscale" },
    relay:    { d: ["healthy", ""], r: ["herdr-core ok", ""], t: ["herdr-tui ok", ""], p: ["live", ""], reach: "relay · e2e", preach: "relay · e2e" },
    offline:  { d: ["healthy · as of 22:08", "warn"], r: ["herdr-core ok", "warn"], t: ["herdr-tui ok", "warn"], p: ["live", "warn"], reach: "offline · last sync 22:08", preach: "offline · last sync 22:08" },
  };
  const hseg = (k, [v, cls]) => `<span class="hseg ${cls}"><span class="hdot"></span>${k ? `<span class="k">${k}</span>` : ""}${esc(v)}</span>`;
  const healthDesk = (S) => {
    const h = HEALTH[S.health];
    return `<div class="health" role="status" aria-label="Health">
      <button class="hseg ${h.d[1]}" data-act="goto" data-to="settings" data-sub="connection"><span class="hdot"></span><span class="k">corrald</span>${esc(h.d[0])}</button>
      ${hseg("", h.r)}${hseg("", h.t)}${hseg("PM", h.p)}</div>`;
  };
  const healthPhone = (S) => {
    const h = HEALTH[S.health];
    const lock = S.health === "relay" ? `<svg class="ic" style="width:11px;height:11px;vertical-align:-1px" aria-hidden="true"><use href="#i-lock"/></svg> ` : "";
    return `<button class="phealth" data-act="goto" data-to="settings" data-sub="connection" aria-label="Health and reach">
      <span class="r1">${hseg("corrald", [h.d[0].replace(" · as of 22:08", ""), h.d[1]])}${hseg("PM", h.p)}<span class="reach ${S.health === "offline" ? "warn" : ""}">${lock}${esc(h.preach)}</span></span>
      <span class="r2">${hseg("", h.r)}${hseg("", h.t)}${S.health === "offline" ? `<span class="reach">2 queued · 1 refused</span>` : ""}</span></button>`;
  };

  const mini = (task) => `<span class="mono dim">${task}</span>`;
  const illus = (txt = "illustrative") => `<span class="illus">${esc(txt)}</span>`;

  // Deterministic QR-like matrix (illustrative, not scannable).
  const qr = () => {
    const n = 29, cell = 1; let rects = ""; let seed = 868;
    const rnd = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
    const finder = (x, y) => `<rect x="${x}" y="${y}" width="7" height="7" fill="#111"/><rect x="${x + 1}" y="${y + 1}" width="5" height="5" fill="#fff"/><rect x="${x + 2}" y="${y + 2}" width="3" height="3" fill="#111"/>`;
    const inF = (x, y) => (x < 8 && y < 8) || (x > n - 9 && y < 8) || (x < 8 && y > n - 9);
    for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) if (!inF(x, y) && (rnd() > 0.52 || (y === 6 && x % 2 === 0) || (x === 6 && y % 2 === 0))) rects += `<rect x="${x}" y="${y}" width="1.02" height="1.02" fill="#111"/>`;
    return `<svg viewBox="0 0 ${n} ${n}" shape-rendering="crispEdges" role="img" aria-label="Pairing QR (illustrative)">${rects}${finder(0, 0)}${finder(n - 7, 0)}${finder(0, n - 7)}</svg>`;
  };

  return { esc, T, SPRITE, ic, mk, rail, row, prow, money, healthDesk, healthPhone, HEALTH, mini, illus, qr, LABEL, STAGES };
})();
