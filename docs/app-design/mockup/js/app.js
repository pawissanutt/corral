// App shell: state, frames (desktop window, phone), router, keyboard, command palette.
const S = window.S = {
  frame: "desktop", theme: "dark", health: "healthy", data: "live",
  screen: "now", sel: "T-890", nowTab: "story", ctxTab: "live", lowTab: "since",
  inboxSel: "q", q: { choice: 0, ev: "diff", state: "open", draft: "" },
  plan: "T-870", planScale: false, planMember: "T-868",
  session: "T-868", sessionView: "transcript", agentsMode: "wall",
  review: { layout: "split" }, tlRange: "today",
  setSub: "devices", pair: 2,
  palette: false, pq: "", pi: 0,
  pstack: [], ptab: "inbox", sheet: null, lock: false,
  toast: null,
  // Ask composer and assistant (see screens-assistant.js)
  ask: { open: false, target: "assistant", chip: null, text: "", model: null, modelMenu: false },
  asst: "answering", stream: 1, lastQ: null, hk: "answer", share: null, sent: [], edit: null, savedNow: false,
};

const DEST = [
  ["inbox", "Inbox", "inbox", "i"], ["now", "Now", "now", "n"], ["changes", "Changes", "changes", "."], ["plans", "Plans", "plans", "p"],
  ["agents", "Agents", "agents", "a"], ["review", "Review", "review", "r"], ["timeline", "Timeline", "timeline", "t"], ["usage", "Usage", "usage", "u"], ["settings", "Settings", "settings", ","],
];
// Needs you = board items (T-868 question, T-885 plan, PM/Q7) + the assistant's own prompts (write permission, mode confirm).
const COUNT = { inbox: () => (S.data === "empty" ? 0 : (S.q.state === "open" && S.data !== "elsewhere" ? 3 : 2) + 2), changes: () => 12, plans: () => 6, agents: () => 7, review: () => 3 };

function connBanner(phone) {
  const queued = `<b>2 queued</b> (answer T-868/R1/Q1 at rev 2, checkable on reconnect; one message to the quick thread) · <b>1 refused</b> (accept plan T-885 P2)`;
  const B = {
    degraded: ["warn", `<b>corrald degraded · sync stalled 4m.</b> The board can trail reality by up to 4 minutes; commands still go through and are revision-checked. PM is relaunching.`, `<button class="btn quiet" data-act="goto" data-to="settings" data-sub="connection">Details</button>`],
    down: ["bad", `<b>corrald is down since 22:09.</b> Showing the board as of 22:08, marked stale. Nothing syncs, gates or lands until it is back.`, `<button class="btn ghost" data-act="toast" data-msg="Restart requested via service manager (systemctl --user restart corrald)">Restart corrald</button><button class="btn quiet" data-act="goto" data-to="settings" data-sub="connection">Diagnostics</button>`],
    offline: ["info", `<b>Offline · last sync 22:08.</b> Reading the last known state. A command queues only if corrald can check it on reconnect; everything else refuses. ${queued}.`, `<button class="btn quiet" data-act="goto" data-to="settings" data-sub="connection">Queue</button>`],
  }[S.health];
  if (!B) return "";
  if (phone) return `<div class="conn-banner ${B[0]}" style="font-size:13.5px;line-height:1.45;padding:10px 18px"><span>${B[1]}</span></div>`;
  return `<div class="conn-banner ${B[0]}"><span>${B[1]}</span><span class="act">${B[2]}</span></div>`;
}

function deskShell(inner) {
  const nav = DEST.map(([k, label, icon, key]) => {
    const n = COUNT[k] ? COUNT[k]() : null;
    const cur = S.screen === k || (k === "inbox" && S.screen === "question") || (k === "agents" && ["session", "athread"].includes(S.screen));
    let h = `<button class="nav" data-act="goto" data-to="${k}" ${cur ? 'aria-current="page"' : ""}>${U.ic(icon)}<span>${label}</span>${n != null ? `<span class="n ${k === "inbox" ? "you" : ""}">${n}</span>` : ""}<span class="key">${key}</span></button>`;
    if (k === "settings") h += `<button class="nav sub" data-act="goto" data-to="settings" data-sub="devices" ${S.screen === "settings" && S.setSub === "devices" ? 'aria-current="page"' : ""}>Devices</button><button class="nav sub" data-act="goto" data-to="settings" data-sub="machines" ${S.screen === "settings" && S.setSub === "machines" ? 'aria-current="page"' : ""}>Machines</button>`;
    return h;
  }).join("");
  const h = U.HEALTH[S.health];
  return `<div class="win ${["down", "offline"].includes(S.health) ? "is-stale" : ""}" role="application" aria-label="Corral desktop">
    <div class="titlebar">
      <span class="app"><svg class="ic brand" aria-hidden="true"><use href="#i-brand"/></svg>Corral</span>
      <span class="ws">~/main-workspace/corral</span>
      ${U.healthDesk(S)}
      <span class="tb-right">
        <button class="reach" data-act="goto" data-to="settings" data-sub="connection" title="Reach path">${S.health === "relay" ? U.ic("lock") : ""}${U.esc(h.reach)}</button>
        <span>▲ 12 since ${D.LAST_VISIT}</span>
        <span class="clock">${D.NOW}</span>
        <span class="winctl"><span>&#x2013;</span><span>&#x25A1;</span><span>&#x2715;</span></span>
      </span>
    </div>
    <div class="wbody">
      <nav class="side" aria-label="Destinations">
        <div class="proj"><div><b>corral</b><span class="dim">Autopilot · independent work continues</span></div></div>
        <button class="ask-hint" data-act="ask-open">${U.ic("chat")}<span>Ask…</span><span class="kbd">/</span></button>
        <button class="palette-hint" data-act="palette">${U.ic("search")}<span>Go to, answer, open…</span><span class="kbd">Ctrl K</span></button>
        ${nav}
        <div class="grow"></div>
        <div class="foot"><span>This device <b>owner-desktop</b></span><span>Actions sign as <span class="mono">via=app:owner-desktop</span></span></div>
      </nav>
      <main class="main">${inner}</main>
    </div>
    ${S.ask.open && S.screen !== "athread" ? ASKUI.drawer() : ""}
    ${S.palette ? palette() : ""}
    ${S.confirm ? confirmDialog(false) : ""}
    ${S.toast ? `<div class="toast" role="status">${S.toast}</div>` : ""}
  </div>`;
}

// Phone tab bar: Inbox, Now, Agents, Plans, More. Ask owns no tab: a floating Ask button sits above the bar (or above a
// screen's decision bar) on every screen and opens the composer sheet with the keyboard up. Hidden where a composer is docked.
const PTABS = [["inbox", "Inbox", "inbox"], ["now", "Now", "now"], ["agents", "Agents", "agents"], ["plans", "Plans", "plans"], ["more", "More", "more"]];
function phoneShell(page) {
  const tabs = PTABS.map(([k, l, i]) => `<button data-act="ptab" data-to="${k}" ${S.ptab === k ? 'aria-current="page"' : ""}>${U.ic(i)}${l}${k === "inbox" && COUNT.inbox() ? `<span class="badge">${COUNT.inbox()}</span>` : ""}</button>`).join("");
  const top = S.pstack.length ? S.pstack[S.pstack.length - 1].page : S.ptab;
  const fab = ["athread", "session"].includes(top) || S.ask.open ? "" : `<div class="fabdock"><button class="askfab" data-act="ask-open" aria-label="Ask (opens the composer)">${U.ic("chat")}Ask</button></div>`;
  const head = page.large && !S.pstack.length
    ? `<div class="plarge"><h1>${page.title}</h1><span class="rt">${page.right || ""}<button data-act="palette" aria-label="Search">${U.ic("search")}</button></span></div>`
    : `<div class="pnav"><button class="back" data-act="pback" aria-label="Back">${U.ic("back")}<span>${page.backLabel || "Back"}</span></button><span class="ttl">${page.title}</span><span class="rt">${page.right || ""}</span></div>`;
  return `<div class="phone ${["down", "offline"].includes(S.health) ? "is-stale" : ""}" role="application" aria-label="Corral phone">
    <div class="island"></div>
    <div class="pstatus"><span>${D.NOW}</span><span class="sig"><svg width="18" height="12" aria-hidden="true"><use href="#i-signal"/></svg><svg width="27" height="13" aria-hidden="true"><use href="#i-batt"/></svg></span></div>
    ${U.healthPhone(S)}
    ${S.share ? `<div class="pbody">${ASKUI.sharePhone()}</div>` : `${head}
    <div class="pbody pv" data-scroll="p-${top}">${connBanner(true)}<div class="stalefx">${page.body}</div></div>
    ${fab}
    ${page.decide ? `<div class="pdecide">${page.decide}</div>` : ""}
    <nav class="ptab" aria-label="Tabs">${tabs}</nav>`}
    <div class="homeind"></div>
    ${S.ask.open && !S.share ? ASKUI.phoneSheet() : ""}
    ${S.sheet ? `<div class="psheet-scrim" data-act="sheet-close"></div><div class="psheet" role="dialog" aria-label="${U.esc(S.sheet.title)}"><div class="grab"></div><div class="sh-h">${S.sheet.title}<button data-act="sheet-close">Done</button></div><div class="sh-b">${S.sheet.body()}</div></div>` : ""}
    ${S.palette ? `<div class="psheet-scrim" data-act="palette-close"></div><div class="psheet" style="max-height:88%">${paletteInner(true)}</div>` : ""}
    ${S.lock ? lockScreen() : ""}
    ${S.confirm ? confirmDialog(true) : ""}
    ${S.toast ? `<div class="toast" style="bottom:100px;white-space:normal;width:340px" role="status">${S.toast}</div>` : ""}
  </div>`;
}

// High-risk commands (land/accept, approve review, grants, unstick, revoke) need a fresh confirmation on this
// device, bound to a preview of the exact effect (confirm.challenge / confirm.response).
const RISK = {
  "accept-plan": { title: "Accept plan T-885 P2", cmd: "corral plan accept T-885 --rev P2", effect: ["Publishes 3 member tasks to main as tracker T-893", "Autopilot may admit pm-bounded-read-contract next", "Gates, independent review and landing checks unchanged"], ok: "Accepted T-885 P2 · 3 members published as T-893" },
  "approve-review": { title: "Approve candidate 85b45e1e", cmd: "corral review approve T-868 --sha 85b45e1e", effect: ["Owner verdict on T-868 R1 at 85b45e1e only", "Landing proceeds to main after the land check", "A newer candidate would make this verdict stale"], ok: "Approved 85b45e1e · landing proceeds" },
  revoke: { title: "Revoke device", cmd: "corral device revoke", effect: ["Retires this device's keys now", "Closes its open sessions and relay route", "Its past actions keep their via= attribution"], ok: "Revoked · keys retired, sessions closed" },
  "mode-pause": { title: "Confirm corral mode pause", cmd: "corral mode pause", effect: ["Autopilot admits no new work after the running gates finish", "Running gates, reviews and landings settle normally", "Cites owner input A-7/g2/i19: \"Pause autopilot for tonight once the running gates finish.\""], ok: "corral mode pause executed · receipt in the quick thread" },
  discard: { title: "Discard", cmd: "corral discard", effect: ["Drops the Idea the assistant filed from your message", "Your words stay in the quick thread", "Nothing else on the board changes"], ok: "Discarded · receipt in the quick thread" },
};
function confirmDialog(phone) {
  const c = RISK[S.confirm.kind], who = S.confirm.n ? ` ${U.esc(S.confirm.n)}` : "";
  const via = phone ? "owner-iphone" : "owner-desktop";
  const preview = `<div class="h3" style="margin-top:4px">Exact effect</div><ul class="steps">${c.effect.map((e) => `<li>${U.mk("na")}<span>${U.esc(e)}</span></li>`).join("")}</ul><div class="mono dim" style="font-size:11.5px">${U.esc(c.cmd)}${who} · actor=owner · via=app:${via} · challenge expires in 60 s</div>`;
  if (phone) return `<div class="psheet-scrim" data-act="confirm-no"></div><div class="psheet" role="dialog" aria-label="${U.esc(c.title)}"><div class="grab"></div><div class="sh-h">${U.esc(c.title)}${who}</div><div class="sh-b">${preview}
    <div style="display:grid;justify-items:center;gap:8px;margin:18px 0 6px"><div style="width:64px;height:64px;border-radius:16px;box-shadow:inset 0 0 0 2px var(--ink-2);display:grid;place-items:center">${U.ic("lock")}</div><span class="dim" style="font-size:14px">Confirm with Face ID, or enter your PIN</span></div>
    <div style="display:flex;gap:8px;margin-top:10px"><button class="pbtn ghost wide" data-act="confirm-no">Cancel</button><button class="pbtn wide" data-act="confirm-yes">Confirm</button></div></div></div>`;
  return `<div class="scrim" style="align-items:center;padding:0" data-act="confirm-no"><div data-stop class="palette" role="dialog" aria-label="${U.esc(c.title)}" style="width:520px;padding:18px 20px 16px">
    <div style="font-weight:700;font-size:15px">${U.esc(c.title)}${who}</div><div class="dim" style="font-size:12.5px;margin-top:2px">Confirm on this device. The daemon checks the preview still matches before it applies.</div>${preview}
    <div style="display:flex;gap:8px;justify-content:flex-end;margin-top:16px"><button class="btn ghost" data-act="confirm-no">Cancel <span class="kbd">Esc</span></button><button class="btn" data-act="confirm-yes">Confirm ${U.esc(c.title.split(" ")[0].toLowerCase())}</button></div></div></div>`;
}

function lockScreen() {
  return `<div class="lock" data-act="lock-off">
    <div class="ld">Saturday 7 November</div><div class="lt">22:12</div>
    <button class="notif" data-act="push-open"><div class="nh"><span class="appi"><svg class="ic" style="width:14px;height:14px;color:white"><use href="#i-brand"/></svg></span>CORRAL<span class="t">now</span></div>
      <div class="nb"><b>Needs you · T-868 host-queue-ledger</b>Restore policy: pane-origin input only, or all origins? Worker recommends option 1.</div></button>
    <div class="lf">Push text is generated on this phone from an encrypted payload; the relay only sends a wake.</div>
  </div>`;
}

// ---------- command palette ----------
function commands() {
  const go = (k, l, key) => ({ g: "Go to", l, key, act: `goto:${k}` });
  const q = S.pq.trim();
  const ask = q && !/^(t-|go |open )/i.test(q) ? [{ g: "Ask", l: `Ask: “${q}”`, key: "Enter", act: `ask:${q}`, always: true }] : [];
  return [
    ...ask,
    { g: "Needs you", l: "Answer T-868/R1/Q1 · restore policy", key: "", act: "goto:question", id: "T-868" },
    { g: "Needs you", l: "Review plan T-885 P2 · accept or request changes", key: "", act: "plan:T-885" },
    ...DEST.map(([k, l, , key]) => go(k, l, key)),
    { g: "Open", l: "T-890 pm-bounded-read-contract", key: "", act: "task:T-890", id: "T-890" },
    { g: "Open", l: "T-894 api-connections", key: "", act: "task:T-894", id: "T-894" },
    { g: "Open", l: "T-871 annotation-service · session", key: "", act: "session:T-871", id: "T-871" },
    { g: "Open", l: "T-868 host-queue-ledger · session", key: "", act: "session:T-868", id: "T-868" },
    { g: "Open", l: "Plan T-870 Pi input queue lifecycle", key: "", act: "plan:T-870" },
    { g: "Device", l: "Pair a new device", key: "", act: "goto:settings" },
    { g: "View", l: "Switch transcript / terminal", key: "`", act: "toggle-term" },
    { g: "View", l: `Theme: ${S.theme === "dark" ? "light" : "dark"}`, key: "", act: "theme" },
  ];
}
function filtered() {
  const q = S.pq.trim().toLowerCase();
  return commands().filter((c) => c.always || !q || (c.g + " " + c.l).toLowerCase().includes(q));
}
function paletteInner(phone) {
  const list = filtered();
  let g = "", items = "";
  list.forEach((c, i) => {
    if (c.g !== g) { g = c.g; items += `<div class="pg">${g}</div>`; }
    items += `<button class="pi" data-act="run" data-cmd="${c.act}" aria-selected="${i === S.pi}" role="option">${U.esc(c.l)}${c.key ? `<span class="kbd">${U.esc(c.key)}</span>` : ""}</button>`;
  });
  if (!list.length) items = `<div class="empty" style="padding:20px 14px"><p>No command or task matches “${U.esc(S.pq)}”. Task ids work too: try <span class="mono">T-894</span>.</p></div>`;
  return `<div class="palette" style="${phone ? "width:auto;box-shadow:none;background:transparent" : ""}" role="dialog" aria-label="Command palette">
    <div class="pin">${U.ic("search")}<input id="pq" placeholder="Go to, open a task id, answer, accept…" value="${U.esc(S.pq)}" autocomplete="off" aria-label="Search commands"></div>
    <div class="pl" role="listbox">${items}</div>
    ${phone ? "" : `<div class="pf"><span><span class="kbd">↑↓</span> move</span><span><span class="kbd">Enter</span> run</span><span><span class="kbd">Esc</span> close</span><span style="margin-left:auto">Command · NSH</span></div>`}
  </div>`;
}
const palette = () => `<div class="scrim" data-act="palette-close"><div data-stop>${paletteInner(false)}</div></div>`;

function runCmd(cmd) {
  const [k] = cmd.split(":"), v = cmd.slice(k.length + 1);
  S.palette = false; S.pq = ""; S.pi = 0;
  if (k === "goto") return nav(v);
  if (k === "ask") { S.ask.open = true; S.ask.text = v; render(); return; }
  if (k === "task") { S.sel = v; return nav("now"); }
  if (k === "plan") { S.plan = v; return nav("plans", { plan: v }); }
  if (k === "session") { S.session = v; return nav("session"); }
  if (k === "toggle-term") { S.sessionView = S.sessionView === "terminal" ? "transcript" : "terminal"; return nav("session"); }
  if (k === "theme") { S.theme = S.theme === "dark" ? "light" : "dark"; }
  render();
}

// ---------- navigation ----------
function nav(to, extra = {}) {
  if (S.frame === "phone") {
    const tabFor = { inbox: "inbox", now: "now", plans: "plans", agents: "agents" };
    if (to === "question") { S.ptab = "inbox"; S.pstack = [{ page: "question" }]; }
    else if (to === "session") { S.ptab = "agents"; S.pstack = [{ page: "session" }]; }
    else if (to === "athread") { S.ptab = "agents"; S.pstack = [{ page: "athread" }]; }
    else if (tabFor[to]) { S.ptab = tabFor[to]; S.pstack = extra.plan ? [{ page: "plan" }] : []; }
    else { S.ptab = "more"; S.pstack = to === "settings" && extra.sub ? [{ page: "settings" }, { page: "settings", sub: extra.sub }] : [{ page: to }]; }
  } else {
    S.screen = to;
  }
  if (extra.sub) S.setSub = extra.sub;
  render();
}

// ---------- render ----------
const scrollMemo = {};
function render() {
  document.documentElement.className = "t-" + S.theme;
  const stage = document.getElementById("stage");
  stage.querySelectorAll("[data-scroll]").forEach((el) => (scrollMemo[el.dataset.scroll] = el.scrollTop));
  let html;
  if (S.frame === "hotkey") {
    html = ASKUI.hotkeyFrame();
  } else if (S.frame === "desktop") {
    const scr = SCREENS[S.screen] || SCREENS.now;
    html = deskShell(scr.desk(S));
  } else {
    const top = S.pstack.length ? S.pstack[S.pstack.length - 1].page : S.ptab;
    const scr = SCREENS[top] || SCREENS.inbox;
    html = phoneShell(scr.phone(S));
  }
  stage.innerHTML = html;
  // Threads open at their newest message; later renders keep the reader's position.
  const BOTTOM = ["drawer", "as-c", "p-athread"];
  stage.querySelectorAll("[data-scroll]").forEach((el) => {
    const k = el.dataset.scroll;
    if (BOTTOM.includes(k) && (scrollMemo[k] == null || scrollMemo["@" + k])) el.scrollTop = el.scrollHeight;
    else if (scrollMemo[k]) el.scrollTop = scrollMemo[k];
    if (BOTTOM.includes(k)) el.onscroll = () => (scrollMemo["@" + k] = el.scrollTop + el.clientHeight >= el.scrollHeight - 8);
  });
  document.querySelectorAll(".studio-bar [data-set]").forEach((b) => b.setAttribute("aria-pressed", String(S[b.dataset.set] === b.dataset.val)));
  document.getElementById("sb-health").value = S.health;
  document.getElementById("sb-data").value = S.data;
  if (S.palette) { const i = document.getElementById("pq"); if (i) { i.focus(); i.setSelectionRange(i.value.length, i.value.length); } }
  else if (S.ask.open || S.frame === "hotkey" || S.share === "compose") { const i = document.getElementById("ask-input"); if (i) { i.focus(); i.setSelectionRange(i.value.length, i.value.length); } }
  document.getElementById("sb-asst").value = S.asst;
}

let toastTimer;
function toast(msg) { S.toast = msg; render(); clearTimeout(toastTimer); toastTimer = setTimeout(() => { S.toast = null; render(); }, 3400); }
window.toast = toast; window.nav = nav; window.render = render;

// ---------- events ----------
document.addEventListener("click", (e) => {
  const stop = e.target.closest("[data-stop]");
  const el = e.target.closest("[data-act]");
  if (!el || (stop && !stop.contains(el) )) return;
  if (stop && el.dataset.act === "palette-close" && stop.contains(e.target) && !e.target.closest(".pi")) return;
  const a = el.dataset.act, d = el.dataset;
  const H = window.ACTIONS[a];
  if (H) H(d, el, e);
});
document.addEventListener("input", (e) => {
  if (e.target.id === "pq") { S.pq = e.target.value; S.pi = 0; const pos = e.target.selectionStart; render(); const i = document.getElementById("pq"); i && i.setSelectionRange(pos, pos); }
  if (e.target.dataset.bind === "qdraft") S.q.draft = e.target.value;
  if (e.target.dataset.bind === "ask") S.ask.text = e.target.value;
});
Object.assign(window.ACTIONS, {
  goto: (d) => { nav(d.to, { sub: d.sub }); },
  palette: () => { S.palette = true; S.pq = ""; S.pi = 0; render(); },
  "palette-close": () => { S.palette = false; render(); },
  run: (d) => runCmd(d.cmd),
  select: (d) => { S.sel = d.id; render(); },
  ptab: (d) => { S.ptab = d.to; S.pstack = []; S.sheet = null; render(); },
  pback: () => { S.pstack.pop(); S.sheet = null; render(); },
  push: (d) => { S.pstack.push({ page: d.page, id: d.id }); if (d.id) { if (d.page === "task") S.sel = d.id; if (d.page === "session") S.session = d.id; if (d.page === "plan") S.plan = d.id; } render(); },
  "open-task": (d) => { if (S.frame === "phone") { S.sel = d.id; S.pstack.push({ page: "task", id: d.id }); render(); } else { S.sel = d.id; nav("now"); } },
  "sheet-close": () => { S.sheet = null; render(); },
  set: (d) => { if (d.k === "plan") S.planScale = false; S[d.k] = d.v === "true" ? true : d.v === "false" ? false : d.v; render(); },
  setq: (d) => { S.q[d.k] = d.k === "choice" ? +d.v : d.v; render(); },
  toast: (d) => toast(U.esc(d.msg)),
  "lock-off": (d, el, e) => { if (!e.target.closest(".notif")) { S.lock = false; render(); } },
  "push-open": () => { S.lock = false; nav("question"); },
  confirm: (d) => {
    if (["offline", "down"].includes(S.health)) return toast("Refused: needs a live connection. Nothing was queued.");
    S.confirm = { kind: d.kind, n: d.n }; render();
  },
  "confirm-no": () => { S.confirm = null; render(); },
  "confirm-yes": () => { const c = RISK[S.confirm.kind], n = S.confirm.n; S.confirm = null; toast(`${U.esc(c.ok)}${n ? " · " + U.esc(n) : ""} · actor=owner via=app:${S.frame === "phone" ? "owner-iphone" : "owner-desktop"}`); },
});

const KEYMAP = { i: "inbox", n: "now", ".": "changes", p: "plans", a: "agents", r: "review", t: "timeline", u: "usage", ",": "settings" };
document.addEventListener("keydown", (e) => {
  const typing = e.target.matches("input, textarea, select");
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") { e.preventDefault(); S.palette = !S.palette; S.pq = ""; S.pi = 0; return render(); }
  if (S.palette) {
    const list = filtered();
    if (e.key === "Escape") { S.palette = false; return render(); }
    if (e.key === "ArrowDown") { e.preventDefault(); S.pi = Math.min(list.length - 1, S.pi + 1); return render(); }
    if (e.key === "ArrowUp") { e.preventDefault(); S.pi = Math.max(0, S.pi - 1); return render(); }
    if (e.key === "Enter" && list[S.pi]) { e.preventDefault(); return runCmd(list[S.pi].act); }
    return;
  }
  if (typing) {
    if (e.target.dataset.bind === "ask") return ASKUI.key(e);
    if (e.key === "Escape") e.target.blur();
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && window.ACTIONS.answer) window.ACTIONS.answer({});
    return;
  }
  if (e.key === "Escape") {
    if (S.confirm) { S.confirm = null; return render(); }
    if (S.ask.modelMenu) { S.ask.modelMenu = false; return render(); }
    if (S.ask.open) { S.ask.open = false; S.ask.chip = null; return render(); }
    if (S.sheet) { S.sheet = null; return render(); }
    if (S.lock) { S.lock = false; return render(); }
    if (S.frame === "phone" && S.pstack.length) { S.pstack.pop(); return render(); }
    if (S.screen === "question") return nav("inbox");
    if (S.screen === "session" || S.screen === "athread") return nav("agents");
    return;
  }
  if (e.key === "?") { S.palette = true; return render(); }
  if (e.key === "/") { e.preventDefault(); return ACTIONS["ask-open"]({}); }
  if (e.key === "`" ) { S.sessionView = S.sessionView === "terminal" ? "transcript" : "terminal"; if (S.frame === "desktop" && S.screen !== "session") S.screen = "session"; return render(); }
  if (S.frame === "desktop" && KEYMAP[e.key]) return nav(KEYMAP[e.key]);
  const scr = S.frame === "desktop" ? SCREENS[S.screen] : null;
  if (scr && scr.keys && scr.keys(e, S) !== false) { e.preventDefault(); render(); }
});

// studio bar
document.querySelector(".studio-bar").addEventListener("click", (e) => {
  const b = e.target.closest("[data-set]"); if (!b) return;
  S[b.dataset.set] = b.dataset.val;
  if (b.dataset.set === "frame") { S.palette = false; S.sheet = null; S.lock = false; if (S.frame === "phone" && !S.pstack.length) S.ptab = "inbox"; }
  render();
});
document.getElementById("sb-health").addEventListener("change", (e) => { S.health = e.target.value; render(); });
document.getElementById("sb-data").addEventListener("change", (e) => { S.data = e.target.value; render(); });
document.getElementById("sb-asst").addEventListener("change", (e) => ACTIONS.asst({ v: e.target.value }));
document.getElementById("sb-lock").addEventListener("click", () => { S.frame = "phone"; S.lock = true; S.share = null; render(); });
document.getElementById("sb-share").addEventListener("click", () => { S.frame = "phone"; S.lock = false; S.ask.open = false; S.share = "sheet"; render(); });

// deep link: #frame/theme/screen
(function boot() {
  const h = location.hash.replace("#", "").split("/").filter(Boolean);
  h.forEach((p) => {
    if (["desktop", "phone", "hotkey"].includes(p)) S.frame = p;
    else if (["dark", "light"].includes(p)) S.theme = p;
    else if (U.HEALTH[p]) S.health = p;
    else if (["loading", "empty", "error", "elsewhere"].includes(p)) S.data = p;
    else if (p === "lock") S.lock = true;
    else if (p === "answered") S.q.state = "elsewhere";
    else if (p === "terminal") S.sessionView = "terminal";
    else if (p === "unified") S.review.layout = "unified";
    else if (p === "scale") S.planScale = true;
    else if (p.startsWith("plan=")) { S.plan = p.slice(5); if (S.plan === "S-900") S.planScale = true; if (S.frame === "phone") { S.ptab = "plans"; S.pstack = [{ page: "plan" }]; } }
    else if (p.startsWith("sub=")) S.setSub = p.slice(4);
    else if (p.startsWith("task=")) { S.sel = p.slice(5); if (S.frame === "phone") S.pstack.push({ page: "task" }); }
    else if (p.startsWith("ev=")) S.q.ev = p.slice(3);
    else if (p === "sheet-thread") S.sheet = "thread";
    else if (p.startsWith("confirm=")) S.confirm = { kind: p.slice(8) };
    else if (p.startsWith("rtab=")) S.review.tab = p.slice(5);
    else if (p === "ask") S.ask.open = true;
    else if (p === "topm") S.ask.target = "pm";
    else if (p.startsWith("askabout=")) { S.ask.open = true; S.ask.chip = decodeURIComponent(p.slice(9)); }
    else if (p.startsWith("asktext=")) S.ask.text = decodeURIComponent(p.slice(8));
    else if (p.startsWith("model=")) S.ask.model = decodeURIComponent(p.slice(6));
    else if (p === "modelmenu") { S.ask.modelMenu = true; if (S.frame !== "hotkey") S.ask.open = true; }
    else if (p.startsWith("asst=")) S.asst = p.slice(5);
    else if (p.startsWith("stream=")) S.stream = +p.slice(7);
    else if (p.startsWith("edit=")) S.edit = p.slice(5);
    else if (p === "savednow") S.savedNow = true;
    else if (p === "withdrawn") S.withdrawn = true;
    else if (p.startsWith("sent=")) { const k = p.slice(5); S.sent = [{ text: k === "queuedpm" ? "hold T-894 if its gate fails twice" : "remind me to look at T-852's op log first thing tomorrow", dev: S.frame === "phone" ? "owner-iphone" : "owner-desktop", pm: k === "queuedpm", state: { queued: "queued on this device", queuedpm: "queued on this device", waiting: "waiting for assistant · host down", sent: "sent" }[k] }]; }
    else if (p.startsWith("hk=")) S.hk = p.slice(3);
    else if (p === "share" || p.startsWith("share=")) S.share = p.includes("=") ? p.slice(6) : "sheet";
    else if (p.startsWith("athread=")) { S.athread = p.slice(8); p = "athread"; }
    if (p === "assistant") p = "agents"; // the Assistant destination folded into Agents
    if (SCREENS[p] || p === "more") {
      if (S.frame === "phone") {
        if (["inbox", "now", "agents", "plans", "more"].includes(p)) { S.ptab = p; }
        else if (p === "question") { S.ptab = "inbox"; S.pstack.push({ page: "question" }); }
        else if (p === "session" || p === "athread") { S.ptab = "agents"; S.pstack.push({ page: p }); }
        else if (p === "settings" && S.setSub && location.hash.includes("sub=")) { S.ptab = "more"; S.pstack.push({ page: "settings" }, { page: "settings", sub: S.setSub }); }
        else { S.ptab = "more"; S.pstack.push({ page: p }); }
      } else S.screen = p;
    }
  });
  if (S.sheet === "thread" && window.ACTIONS.thread) { S.sheet = null; window.ACTIONS.thread({}); return; }
  render();
})();
