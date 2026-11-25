// Ask: one chat to an agent (the assistant, or the PM). The quick thread and other assistant sessions live in Agents;
// filed ideas, global-hotkey window, phone share sheet.
// REAL: assistant model (A-<n> sessions, owner inputs A-<n>/g<gen>/i<seq>, owner-action kinds and classes,
//       receipts executed|refused|declined, ask_user, write-permission choices) from docs/guide/assistants.md;
//       board facts the answer cites (snapshot 22:12); T-874, T-852 titles. ILLUSTRATIVE: every message text,
//       timings, costs, PMI ids, filed ids T-899..T-905, sessions, refusal texts, paths, the shared page.
(() => {
  const { esc, ic, mk, illus } = U;
  const dev = () => (S.frame === "phone" ? "owner-iphone" : "owner-desktop");
  const off = () => ["offline", "down"].includes(S.health);
  const L = (label, act, extra = "") => `<a class="ln2" data-act="${act}" ${extra}>${esc(label)}</a>`;
  const task = (id) => L(id, "open-task", `data-id="${id}"`);
  // Why the assistant cannot run right now (owner message then shows `waiting for assistant · <reason>`).
  const WAIT = { down: "host down", load: "machine load", quota: "provider quota" };
  const waitReason = () => WAIT[S.asst];
  // Model selector (D22): profiles from `[assistant.quick] models`; the first is the assistant profile. The choice is
  // thread-wide, applies to the next message and keeps the conversation. Profile names illustrative; model ids real.
  const MODELS = [["pi-sol xhigh", "gpt-6.1-sol · xhigh", "assistant profile · default"], ["pi-terra high", "gpt-5.6-terra · high", "cheaper"], ["pi-luna", "gpt-6-luna · low", "cheapest · also writes roll summaries"], ["pi-opus high", "claude-opus-4-5 · high", "Anthropic"]];
  const model = () => S.ask.model || MODELS[0][0];
  const modelMenu = (cls) => `<div class="modelmenu ${cls}" role="listbox" aria-label="Model">${MODELS.map(([n, m, note]) => `<button role="option" aria-selected="${n === model()}" data-act="ask-model" data-v="${n}"><span class="mck">${n === model() ? "✓" : ""}</span><span class="mn"><b>${n}</b><span class="mono dim">${m}</span></span><span class="dim mnote">${note}</span></button>`).join("")}<div class="mmfoot">Applies to your next message; the conversation is kept. List: <span class="mono">[assistant.quick] models</span> · Settings › Assistant ${illus("profile names illustrative")}</div></div>`;
  const modelPick = (cls) => `<span class="pickwrap"><button class="pick" data-act="ask-model-menu" aria-haspopup="listbox" aria-expanded="${!!S.ask.modelMenu}" title="Model for the next message">${esc(model())} ▾</button>${S.ask.modelMenu && cls !== "inline" ? modelMenu(cls) : ""}</span>`;

  const BOTH_Q = "why is T-867 stale, and what's blocking T-870? we should cache gate receipts so a base move doesn't force a full re-gate";
  const ANSWER = [
    ["p", [task("T-867"), " is stale because the base moved. Its gate ran on candidate ", L("5949a4b", "open-task", 'data-id="T-867"'), "; main moved at 22:05, so that verdict now belongs to an older candidate. Corral queued a re-gate on the rebased candidate, so nothing is needed from you there."]],
    ["p", ["Plan ", L("T-870", "plan-open", 'data-id="T-870"'), " has three blockers:"]],
    ["ol", [
      [task("T-867"), " itself: both ", task("T-868"), " and ", task("T-869"), " build on it."],
      [task("T-868"), " waits on your answer to ", L("T-868/R1/Q1", "goto", 'data-to="question"'), " (restore policy). The worker recommends option 1."],
      ["None of its 15 acceptance scenarios is mapped to a member, so final verification has no owner yet."],
    ]],
    ["p", ["Your caching idea is filed as ", task("T-903"), ". I checked duplicates first: ", task("T-852"), " handles the same trigger (a gate gone stale after a base move) but does not cache, so I filed it separately."]],
  ];
  const SOURCES = [["corral show T-867", "open-task", 'data-id="T-867"'], ["run R1 of T-867", "open-session", 'data-id="T-867"'], ["corral plan progress T-870", "plan-open", 'data-id="T-870"'], ["corral question show T-868/R1/Q1", "goto", 'data-to="question"'], ["corral list --search receipt", "toast", 'data-msg="duplicate search: 2 candidates, none close"']];
  const TOOLS = [["Bash", "corral show T-867 --json", "0.4 s"], ["Bash", "corral log T-867 --since 20:40", "0.2 s"], ["Bash", "corral plan progress T-870", "0.6 s"], ["Bash", "corral question show T-868/R1/Q1", "0.3 s"], ["Bash", "corral list --search \"gate receipt\" --json", "0.3 s"], ["Bash", "corral new --idea --project corral …", "0.5 s"]];

  // Ideas the assistant filed from chat (owner words verbatim; everything else written by the assistant).
  const FILED = {
    "T-901": { at: "22:04", dev: "owner-desktop", words: "review diffs need an inline comment box tied to the candidate SHA", title: "Review: inline comments bound to the candidate SHA", labels: ["review", "ui"], related: "T-874", criteria: ["A comment on a diff line records the candidate SHA it was written against.", "A newer candidate shows the comment as stale, not deleted.", "Comments appear in the Review screen on desktop and phone."], dup: "asked first: close to T-874; you chose to file separately (related link kept)" },
    "T-902": { at: "22:10", dev: "owner-iphone", words: "board: show the PM inbox count in the title bar", title: "Board: PM inbox count in the title bar", labels: ["board", "pm"], criteria: ["The title bar shows open PM inbox items as a count next to the health line.", "The count updates within one sync pass."], dup: "checked: nothing close" },
    "T-903": { at: "22:11", dev: "owner-desktop", words: "we should cache gate receipts so a base move doesn't force a full re-gate", title: "Gate: reuse receipts for unchanged crates after a base move", labels: ["gate", "performance"], criteria: ["A re-gate after a base move that touches no crate in the candidate's write scope reuses the earlier receipts and records why.", "A base move that touches any crate in scope runs the full tier.", "The new gate receipt names every reused receipt id."], dup: "checked: T-852 shares the trigger, not the fix" },
  };
  // [id, title, when, source session]. One row comes from a pane session (corral pi) to show the source.
  const FILED_LIST = [["T-903", FILED["T-903"].title, "22:11", "A-7 · quick thread"], ["T-902", FILED["T-902"].title, "22:10", "A-7 · quick thread"], ["T-874", "merged · comment on Pi review annotations", "22:04", "A-7 · quick thread"], ["T-899", "PM read view lists assistant inbox items", "yesterday", "A-17 · pane w9A:p1 (corral pi)"]];
  // A named session bound to a task reads `open · task done` once the task is Done or Dropped (host suspended, resumable).
  const SESSIONS = [["A-7", "Quick thread", "live", "runs: daemon", "—", 1.12, 0], ["A-15", "T-854 co-work check", "live", "pane w15C:p2 · corral pi --task T-854", "T-854", 2.18, 0], ["A-17", "Board TUI mockup: async loading and settings", "open", "pane w9A:p1 · corral pi", "—", 6.40, 1], ["A-12", "Co-work C2 follow-ups", "open · task done", "pane w9A:p2 · corral pi --task T-853", "T-853", 0.88, 0], ["A-11", "PM read contract questions", "ended", "—", "T-890", 0.52, 0]];

  // ---------- thread pieces ----------
  const stTag = (s) => s ? `<span class="mstate ${/queued|waiting|late/.test(s) ? "wait" : ""}">${esc(s)}</span>` : "";
  const me = (at, d, text, state = "sent", extra = "") => `<div class="tm"><span class="av o">Y</span><div><div class="hd"><b>you</b> · <span class="mono">app:${d}</span> · ${at}${extra} ${stTag(state)}</div><p>${esc(text)}</p></div></div>`;
  const asst = (at, html, state = "") => `<div class="tm"><span class="av w">A</span><div><div class="hd"><b>assistant A-7</b> · ${at} · <span class="dim">pi-sol xhigh</span>${state}</div>${html}</div></div>`;
  const pmState = (s) => `<span class="pmstate ${s === "PM replied" ? "ok" : s === "delivered" ? "" : "wait"}">${s}</span>`;
  // PM target = an owner-authored PM inbox item (PMI-<n>); the PM's reply is that item's outcome, folded into the thread.
  const pm = (at, d, text, s, pmi, reply) => `<div class="tm"><span class="av o">Y</span><div><div class="hd"><b>you</b> → <b>PM</b> · <span class="mono">${pmi}</span> · <span class="mono">app:${d}</span> · ${at} ${pmState(s)}</div><p>${esc(text)}</p>
    ${reply ? `<div class="pmreply"><div class="hd"><b>PM</b> · ${reply[0]} · outcome of ${pmi}</div>${esc(reply[1])}</div>` : s === "queued for PM" ? `<div class="dim" style="font-size:12px">Delivered when the PM next takes input; supervision is not interrupted.</div>` : ""}</div></div>`;

  function actCard(cmd, cls, input, outcome, opt = {}) {
    // Class: runs on your instruction (dispatch, queue, park, verdicts) | confirm on this device (approve/land, plan_accept,
    // revert, finish, confirm-class kinds, stale or tainted turns). "off": this device may ask and file only.
    const clsTxt = cls === "run" ? "runs on your instruction" : cls === "off" ? "this device may ask and file only" : cls === "late" ? "late · cannot instruct" : "confirm on this device";
    const why = opt.why ? `<span class="dim">${esc(opt.why)}</span>` : "";
    const rc = outcome === "waiting" ? `<div class="rcpt wait"><span class="stamp void">WAITING ON YOU</span><span>Nothing runs until you confirm here; Decline records a <b>declined</b> receipt.</span><span class="acts"><button class="btn" data-act="confirm" data-kind="${opt.kind}">Confirm on this device</button><button class="btn ghost" data-act="toast" data-msg="Declined · receipt recorded · nothing changed">Decline</button></span></div>`
      : outcome === "off" ? `<div class="rcpt"><span>Nothing ran. <span class="mono">owner-laptop</span> is a web client: it may ask and file, not instruct. Say it from the desktop, phone or terminal, or turn instructing on for it in Settings › Devices.</span></div>`
      : outcome === "late" ? `<div class="rcpt"><span>Nothing ran. You typed this at ${opt.typed}; it reached corrald at ${opt.delivered}, more than 15 min later, so it cannot instruct. Say it again to act.</span></div>`
      : `<div class="rcpt ${outcome}"><span class="stamp ${outcome === "executed" ? "" : outcome === "refused" ? "bad" : "void"}">${outcome.toUpperCase()}</span><span class="mono">${esc(opt.excerpt || "")}</span></div>`;
    return `<div class="actcard"><div class="ah"><span class="dim">Owner action</span><span class="chip ${cls === "confirm" ? "you" : ""}">${clsTxt}</span>${why}<span class="mono dim" style="margin-left:auto">cites ${input}</span></div>
      <div class="acmd mono">${esc(cmd)}</div>${opt.confirmed ? `<div class="dim" style="font-size:12px;padding:0 12px 6px">confirmed on ${opt.confirmed}</div>` : ""}${rc}</div>`;
  }
  // Duplicate check: the assistant asks before filing when a duplicate is likely (ask_user, two options with consequences).
  const dupAsk = () => `<div class="askuser"><div class="hd"><span class="dot c-you"></span><b>assistant asks</b> · ask_user · before filing · 22:04</div>
    <p>This looks close to ${task("T-874")} <span class="dim">Pi review annotations: durable candidate-bound threads</span>. Merge, or file separately?</p>
    <div class="opts2">
      <button class="on"><b>Merge into T-874</b><span>Your words become a comment on T-874. Nothing new is filed.</span></button>
      <button disabled><b>File separately</b><span>A new Idea, linked to T-874 as related. It can be merged later.</span></button></div>
    <div class="dim" style="font-size:12px;margin-top:6px">Answered inline by you after 40 s · <span class="mono">A-7/g2/i18</span> · dialog</div></div>`;
  // Merge: nothing new is filed; the owner's words are appended to the existing task as a comment.
  const mergedCard = () => `<div class="filed"><div class="ideahd"><span class="chip">merged · T-874</span><span class="dim">comment added to ${task("T-874")} Pi review annotations: durable candidate-bound threads</span></div>
      <blockquote class="words"><span class="wl">appended as a comment, your words unchanged</span>“${esc(FILED["T-901"].words)}”</blockquote>
      <div class="dim" style="font-size:11.5px">added by assistant A-7 from your message · nothing new filed</div></div>`;
  const promptTimer = `<div class="dim" style="font-size:11.5px;margin-top:6px">The tool waited 120 s, then deferred; this prompt stays answerable here and in Inbox until 22:05 tomorrow. One answer wins.</div>`;
  const perm = () => `<div class="permcard"><div class="hd"><span class="dot c-you"></span><b>assistant wants to write</b> · edit · 22:05 <span class="chip you" style="margin-left:auto">waiting on you</span></div>
    <div class="mono" style="font-size:12px;margin:4px 0">~/main-workspace/notes/corral-log.md · replacement 1 · +6 lines</div>
    <div class="diff" style="font-size:11px;margin:6px 0"><div class="ln add"><span></span><span>41</span><span class="s">+</span><span class="code">## 2026-11-07 night</span></div><div class="ln add"><span></span><span>42</span><span class="s">+</span><span class="code">- T-867 stale on 5949a4b (base moved), re-gate queued</span></div><div class="ln add"><span></span><span>43</span><span class="s">+</span><span class="code">- T-870 waits on T-868/R1/Q1</span></div></div>
    <div class="permbtns"><button class="btn" data-act="toast" data-msg="Allowed once · permission input recorded">Allow once</button><button class="btn ghost" data-act="toast" data-msg="${S.frame === "phone" ? "Face ID confirmed · allowed" : "Allowed"} for 1 hour on ~/main-workspace/notes">Allow for 1 hour</button><button class="btn ghost" data-act="toast" data-msg="${S.frame === "phone" ? "Face ID confirmed · allowed" : "Allowed"} for this session on ~/main-workspace/notes">Allow for this session</button><button class="btn danger" data-act="toast" data-msg="Denied · recorded · not asked again this turn">Deny</button></div>
    <div class="dim" style="font-size:11.5px;margin-top:4px">On a phone, the two standing grants ask for Face ID or PIN first; Allow once and Deny do not.</div>
    <div class="illus" style="margin-top:4px">path and preview illustrative</div></div>`;

  // Filed card: your words verbatim (quoted), then what the assistant wrote; Edit, Merge into T-…, Discard (confirm-class).
  function filedCard(id, opt = {}) {
    const f = FILED[id], edit = S.edit === id;
    return `<div class="filed"><div class="ideahd"><span class="chip ok">filed · ${id}</span><span class="dim">Idea · project <b>corral</b> · <span class="mono">via=app:${f.dev}</span></span>${opt.attach ? `<span class="chip">${ic("review")} link attached</span>` : ""}</div>
      <div class="dim" style="font-size:11.5px;margin-top:4px">filed by assistant A-7 from your message · your words kept as the request</div>
      <blockquote class="words"><span class="wl">your words, unchanged</span>“${esc(f.words)}”</blockquote>
      <dl class="kv fk"><dt>Title</dt><dd>${edit ? `<input class="einput" value="${esc(f.title)}" aria-label="Title">` : `<b>${esc(f.title)}</b>`}</dd>
        <dt>Project</dt><dd>${edit ? `<span class="pick">corral ▾</span>` : "corral"}</dd><dt>Labels</dt><dd>${f.labels.map((l) => `<span class="chip">${l}</span>`).join(" ")}${f.related ? ` <span class="dim">· related</span> ${task(f.related)}` : ""}</dd>
        <dt>Criteria</dt><dd><ol class="crit">${f.criteria.map((c) => `<li>${edit ? `<input class="einput" value="${esc(c)}">` : esc(c)}</li>`).join("")}</ol></dd>
        <dt>Duplicates</dt><dd class="dim">${esc(f.dup)}</dd></dl>
      <div class="sugbtns">${edit ? `<button class="btn" data-act="edit" data-v="">Save</button><button class="btn quiet" data-act="edit" data-v="">Cancel</button>` : `<button class="btn ghost" data-act="edit" data-v="${id}">Edit</button><button class="btn ghost" data-act="toast" data-msg="Merge ${id} into: type a task id (T-852, T-874…) · your words move as a comment">Merge into T-…</button><button class="btn quiet danger-t" data-act="confirm" data-kind="discard" data-n="${id}">Discard</button><span class="dim" style="font-size:11.5px;align-self:center">Discard asks on this device</span>`}</div></div>`;
  }
  // "File as idea now": filed by you, as written; the title is the first line of your words, cut at 120 characters.
  const savedAsWritten = (id) => `<div class="filed"><div class="ideahd"><span class="chip ok">saved · ${id}</span><span class="dim">Idea · filed by you, as written · no model ran</span></div><blockquote class="words"><span class="wl">your words, unchanged</span>“${esc(BOTH_Q)}”</blockquote><dl class="kv fk"><dt>Title</dt><dd><b>${esc(BOTH_Q.split("\n")[0].slice(0, 120))}${BOTH_Q.length > 120 ? "…" : ""}</b> <span class="dim">first line of your words, cut at 120 characters</span></dd></dl><div class="dim" style="font-size:12px;margin-top:4px">No labels or criteria yet. Ask the assistant to refine it when it can run again.</div></div>`;

  function answer(phone) {
    const st = S.asst;
    if (WAIT[st]) {
      const why = { down: "A-7's host stopped (provider exited).", load: "The machine-load probe reports memory below its minimum (illustrative: 1.1 GB free, 2 GB needed).", quota: "The OpenAI subscription window is exhausted until 23:58 (illustrative)." }[st];
      if (S.withdrawn) return `<div class="tm"><span></span><div class="dim" style="font-size:12.5px">withdrawn by you · nothing was filed or run · the text stays here for reference</div></div>`;
      return S.savedNow ? `<div class="tm"><span></span><div>${savedAsWritten("T-903")}</div></div>` : `<div class="waitbox">${mk("stale")}<span><b>waiting for assistant · ${WAIT[st]}</b> · ${why} Your message is stored and every device shows it; the assistant takes it as soon as it can run.</span>
        <span class="acts">${st === "down" ? `<button class="btn ghost" data-act="asst" data-v="answering">Resume A-7</button>` : ""}<button class="btn quiet" data-act="file-now">File as idea now</button><button class="btn quiet" data-act="withdraw">Withdraw</button></span></div>
        <div class="dim" style="font-size:11.5px;margin:-4px 16px 8px 52px">It waits until the assistant can run, you file it, or you withdraw it. It never expires and corrald never files it by itself.</div>`;
    }
    if (st === "starting") return asst("22:11", `<div class="hoststate">${mk("active")}<span><b>starting</b> · A-7's host is starting (first use, or corrald restarted, or the host exited), usually 4–8 s. Your message is stored; keep typing.</span></div>`, ` <span class="chip corral">starting</span>`);
    const total = ANSWER.length + 1;
    const shown = st === "stopped" ? 2 : Math.max(1, Math.ceil(S.stream * total));
    let body = "";
    ANSWER.slice(0, shown).forEach(([k, parts]) => { body += k === "ol" ? `<ol class="aol">${parts.map((li) => `<li>${li.join("")}</li>`).join("")}</ol>` : `<p>${parts.join("")}</p>`; });
    const streaming = st === "answering" && S.stream < 1;
    if (streaming) body += `<span class="caret"></span>`;
    const tools = `<details class="toolsum"><summary>${mk("done")} Read 4 records · searched duplicates · filed 1 Idea · 2.1 s <span class="dim">· tool activity</span></summary>${TOOLS.map(([k, a, t]) => `<div class="tl2"><span class="tk">${k}</span><span class="ta mono">${esc(a)}</span><span class="dim">${t}</span></div>`).join("")}</details>`;
    const foot = `<div class="afoot">${streaming ? `<button class="btn ghost" style="height:26px" data-act="asst" data-v="stopped">Stop</button>` : ""}<span class="dim">this ask $0.04 · 7.3k tokens</span>${st === "stopped" ? `<span class="chip">you stopped this turn · partial answer kept · host still live</span>` : ""}<span class="dim" style="margin-left:auto">${illus("wording illustrative; facts from the 22:12 snapshot")}</span></div>`;
    const done = !streaming && st !== "stopped";
    const src = done ? `<div class="rests"><span class="dim">Rests on</span> ${SOURCES.map(([l, a, x]) => L(l, a, x)).join(" · ")}</div>` : "";
    return asst("22:11", tools + `<div class="abody">${body}</div>` + (done ? filedCard("T-903") : "") + src + foot, ` <span class="chip ${streaming ? "corral" : ""}">${streaming ? "answering" : st === "stopped" ? "stopped" : "answered and filed"}</span>`);
  }

  // Messages sent in this session of the mock; replies to them are not simulated.
  const sentHtml = () => (S.sent || []).map((m) => m.pm
    ? pm("now", m.dev, m.text, m.state, m.state === "queued on this device" ? "PMI on arrival" : "PMI-121") + (m.state === "queued on this device" ? `<div class="queuednote"><span>Posted to the PM on arrival, with the time you typed it (22:13).</span><span class="acts"><button class="btn quiet" data-act="withdraw">Withdraw</button></span></div>` : "")
    : me("now", m.dev, m.text, m.state) + (m.state === "sent" ? asst("now", `<p class="dim">${illus("mock: the assistant's reply to new messages is not simulated")}</p>`) : m.state.startsWith("waiting") ? `<div class="waitbox" style="margin:0 16px 8px 52px">${mk("stale")}<span>${esc(m.state)}</span><span class="acts"><button class="btn quiet" data-act="file-now">File as idea now</button><button class="btn quiet" data-act="withdraw">Withdraw</button></span></div>` : m.state.startsWith("saved") || m.state === "withdrawn" ? "" : `<div class="queuednote"><span>Delivered once when corrald is reachable; it never expires. If that is more than 15 min from now it can be answered and filed, not instruct. A context chip that changes meanwhile arrives marked "context changed since you typed".</span><span class="acts"><button class="btn quiet" data-act="file-now">File as idea now</button><button class="btn quiet" data-act="withdraw">Withdraw</button></span></div>`)).join("");

  function thread(phone) {
    const waiting = WAIT[S.asst];
    const q = S.lastQ || BOTH_Q;
    return `<div class="tx qthread">
      <div class="sumrow">${mk("na")} earlier conversation summarized · A-6 → A-7 · 14 messages, 3 filed, 2 owner actions <a class="ln2" data-act="toast" data-msg="A-6 transcript opened (read-only)">A-6 transcript</a></div>
      ${me("21:31", "owner-desktop", "queue T-873")}
      ${asst("21:31", actCard("corral queue add T-873", "run", "A-7/g2/i14", "executed", { excerpt: "T-873 annotation-pi-host-api queued · after T-871" }))}
      ${pm("21:44", "owner-iphone", "Hold T-852's re-gate until the external op finishes.", "PM replied", "PMI-118", ["21:46", "Already held (held · external op). I re-gate when the op ends and tell you if it fails again."])}
      ${me("21:47", "owner-iphone", "why is T-852 held? park it till the op is done", "late · cannot instruct", " · typed 21:20 offline")}
      ${asst("21:47", `<p>${task("T-852")} is <b>held · external op</b>: the op started 19:28 and Corral re-gates when it ends. It has spent $43 over 2 rounds.</p>` + actCard("corral attention park T-852", "late", "A-7/g2/i15", "late", { typed: "21:20", delivered: "21:47" }))}
      ${pm("21:50", "owner-desktop", "After T-890 lands, admit T-891 before T-895.", "delivered", "PMI-119")}
      ${me("21:52", "owner-laptop", "queue T-872")}
      ${asst("21:52", actCard("corral queue add T-872", "off", "A-7/g2/i16", "off"))}
      ${me("21:58", "owner-desktop", "land T-890")}
      ${asst("21:58", actCard("corral approve T-890", "confirm", "A-7/g2/i17", "refused", { why: "approve (land) waits for a confirmation", confirmed: "owner-desktop 21:58", excerpt: "approve: T-890 has no passing gate on candidate 1751bd2 (quick gate running)" }) + `<p>The quick gate on 1751bd2 is still running (412/980). I will not retry; say "land T-890" again once it passes.</p><div class="illus">refusal text illustrative</div>`)}
      ${me("22:02", "owner-iphone", "Pause autopilot for tonight once the running gates finish.")}
      ${asst("22:02", actCard("corral mode pause", "confirm", "A-7/g2/i19", "waiting", { why: "mode is confirm-class", kind: "mode-pause" }), ` <span class="chip you">waiting on you</span>`)}
      ${me("22:04", "owner-desktop", FILED["T-901"].words)}
      ${asst("22:04", dupAsk() + mergedCard())}
      ${me("22:05", "owner-desktop", "keep tonight's notes in ~/main-workspace/notes/corral-log.md")}
      ${asst("22:05", perm() + promptTimer, ` <span class="chip you">waiting on you</span>`)}
      ${pm("22:06", "owner-desktop", "Summarise tonight's stalls for me at 07:00.", "queued for PM", "PMI-120")}
      ${me("22:08", "owner-iphone", "read this and dispatch T-891 if it fits", "sent", ` · <span class="chip">link nsh.dev/elements/diff-view</span>`)}
      ${asst("22:08", actCard("corral dispatch T-891", "confirm", "A-7/g2/i21", "declined", { why: "this turn read a shared link", excerpt: "declined on owner-iphone 22:09 · nothing changed" }))}
      ${me("22:10", "owner-iphone", FILED["T-902"].words)}
      ${asst("22:10", filedCard("T-902"))}
      ${me("22:11", dev(), q, waiting ? (S.withdrawn ? "withdrawn" : S.savedNow ? "saved · T-903" : `waiting for assistant · ${waiting}`) : "sent")}
      ${answer(phone)}
      ${sentHtml()}
    </div>`;
  }

  // ---------- composer: one field, one Send; target Assistant | PM ----------
  const CHIPS = { "T-890": ["task", "T-890 pm-bounded-read-contract"], "T-868/R1/Q1": ["question", "T-868/R1/Q1 restore policy"], "hunk": ["diff hunk", "85b45e1e · cross-lane.test.ts @@ -212"], "T-870": ["plan", "plan T-870"], "T-854": ["session", "A-15 · T-854"] };
  // `sess`: a named session's own composer (no Assistant | PM switch; it talks to that session).
  function composer(phone, sess) {
    const a = S.ask;
    const pmS = S.health === "offline" ? "live · as of 22:08" : U.HEALTH[S.health].p[0];
    const cd = a.chip ? CHIPS[a.chip] || ["task", `${a.chip} ${(D.TASKS[a.chip] || {}).title || ""}`] : null;
    const chip = cd ? `<span class="achip">${ic("plans")}<span class="dim">${cd[0]}</span> ${esc(cd[1])}<button data-act="ask-unchip" aria-label="Remove context">${ic("close")}</button></span>` : "";
    const note = off() ? `<div class="savedline"><span class="chip held">offline</span> Messages queue on this device and send once when corrald is reachable.</div>`
      : WAIT[S.asst] && a.target !== "pm" ? `<div class="savedline"><span class="chip held">waiting for assistant · ${WAIT[S.asst]}</span> Send still stores your message; it waits in the thread.</div>` : "";
    const ph = sess ? `Message ${sess}…` : a.chip ? `Ask about ${a.chip}…` : a.target === "pm" ? "Message the PM…" : "Ask a question, drop an idea, or give an instruction…";
    // Daemon-hosted sessions get the model selector; pane sessions keep Pi's own picker; the PM runs its own profile.
    const daemonSess = sess && (OTHER.find((x) => x[0] === sess) || [])[3] === "runs: daemon";
    const pickable = sess ? daemonSess : a.target !== "pm";
    const menuInline = phone && pickable && S.ask.modelMenu ? modelMenu("inline") : "";
    const mnote = pickable && model() !== MODELS[0][0] ? `<div class="savedline"><span class="chip">${esc(model())}</span> Your next message is answered by ${esc((MODELS.find((m) => m[0] === model()) || MODELS[0])[1])}. The conversation is kept.</div>` : "";
    return `<div class="composer2 ${phone ? "ph" : ""}">
      ${sess ? "" : note}${mnote}
      ${sess ? (daemonSess ? `<div class="crow">${modelPick(phone ? "inline" : "up")}<span class="dim" style="font-size:11.5px">model for this session</span></div>${menuInline}` : "") : `<div class="crow">
        <div class="seg" role="radiogroup" aria-label="Send to"><button data-act="ask-target" data-v="assistant" aria-pressed="${a.target === "assistant"}">Assistant</button><button data-act="ask-target" data-v="pm" aria-pressed="${a.target === "pm"}">PM <span class="pmdot ${pmS.split(" ")[0]}"></span><span class="dim">${pmS}</span></button></div>
        ${a.target === "pm" ? `<span class="dim" style="font-size:11.5px">the PM runs its own profile</span>` : modelPick(phone ? "inline" : "up")}
        ${phone ? "" : `<span class="dim ckeys"><span class="kbd">↵</span> send · <span class="kbd">⇧ ↵</span> new line · <span class="kbd">Ctrl ⇧ P</span> PM · <span class="kbd">Ctrl ⇧ M</span> model</span>`}
      </div>${menuInline}`}
      ${chip ? `<div class="chiprow">${chip}<span class="dim" style="font-size:11.5px">the assistant gets a bounded brief of it</span></div>` : ""}
      <textarea id="ask-input" data-bind="ask" rows="${phone ? 3 : 2}" placeholder="${esc(ph)}" aria-label="Ask">${esc(a.text)}</textarea>
      <div class="crow foot">
        <button class="att" data-act="toast" data-msg="Attach a screenshot">${ic("review")}Screenshot</button><button class="att" data-act="toast" data-msg="Attach a link">${ic("chev")}Link</button><button class="att" data-act="toast" data-msg="Attach a file">${ic("plans")}File</button>
        <span class="grow"></span>
        <button class="btn" data-act="ask-send">Send<span class="kbd">↵</span></button>
      </div>
    </div>`;
  }
  const stateChip = () => {
    const s = WAIT[S.asst] ? `waiting for assistant · ${WAIT[S.asst]}` : S.asst === "answering" ? (S.stream < 1 ? "answering" : "waiting on you") : S.asst;
    return `<span class="chip ${s === "answering" || s === "starting" ? "corral" : s.startsWith("waiting for") ? "held" : s === "waiting on you" ? "you" : ""}">${s}</span>`;
  };

  // ---------- desktop drawer ----------
  const drawer = () => `<aside class="drawer" role="dialog" aria-label="Ask">
      <div class="dhead"><b>Quick thread · A-7</b>${stateChip()}<span class="dim" style="font-size:11.5px">shared by your devices · runs in corrald</span><span class="grow"></span><button class="btn quiet" data-act="ask-full">Open in Agents</button><button class="btn quiet" data-act="ask-close" aria-label="Close (Esc)" title="Close (Esc)">${ic("close")}</button></div>
      <div class="dthread" data-scroll="drawer">${thread(false)}</div>
      ${composer(false)}</aside>`;

  // ---------- inside Agents: the quick thread first, other assistant sessions, Filed from chat ----------
  // Hosts never stop for being idle: a session reads `live · since HH:MM` until its work is done or it stops on its own.
  const lastLine = () => WAIT[S.asst] ? `waiting for assistant · ${WAIT[S.asst]}` : S.asst === "starting" ? "starting" : S.asst === "stopped" ? "you stopped the last turn" : S.stream < 1 ? "answering…" : "Your caching idea is filed as T-903. I checked duplicates first…";
  const qtTile = () => `<button class="tile qt" data-act="open-athread" data-id="A-7">
      <div class="th">${stateChip()}<span class="id">A-7</span><span class="dim" style="margin-left:auto">assistant · pinned</span></div>
      <div class="tt2">Quick thread · A-7</div>
      <div class="doing">${esc(lastLine())}</div>
      <div class="qtlast"><span class="dim">you · 22:11</span> ${esc((S.lastQ || BOTH_Q).slice(0, 96))}…</div>
      <div class="tf"><span>${esc(model())}</span><span>runs: daemon</span><span>shared by your devices</span><span class="r">live · since 14:02</span></div></button>`;
  const qtRow = () => `<button class="prow qtrow" data-act="open-athread" data-id="A-7"><span class="dot" style="color:var(--corral);grid-row:1"></span><span class="l1"><span class="id">A-7</span><span class="tt">Quick thread</span></span><span class="r2">${stateChip()}</span><span class="l2"><span class="nw">${esc(lastLine())}</span><span class="r">pinned</span></span></button>`;
  const OTHER = SESSIONS.filter((s) => s[0] !== "A-7").concat([["A-18", "Changes review notes", "live", "runs: daemon", "—", 0.41, 0]]);
  const since = { "A-15": "21:02", "A-18": "20:31" };
  const sessState = (st, id) => (st === "live" ? `live · since ${since[id]}` : st);
  const sessionsBlock = (phone) => `${phone ? `<div class="ph-sec">Assistant sessions<span class="n">${OTHER.length}</span><button class="r" data-act="toast" data-msg="New session A-19 started in corrald · separate from the quick thread">+ New session</button></div>` : `<div class="h3" style="margin-top:0">Assistant sessions <span class="r">${OTHER.length} · daemon and pane</span></div>`}
    ${OTHER.map(([id, t, st, where, focus, cost, scr]) => phone
      ? `<button class="prow" data-act="open-athread" data-id="${id}"><span class="dot" style="color:var(--${st === "live" ? "worker" : st === "open" ? "ink-3" : "ink-4"});grid-row:1"></span><span class="l1"><span class="id">${id}</span><span class="tt">${esc(t)}</span></span><span class="mono dim" style="font-size:12px">$${cost.toFixed(2)}</span><span class="l2"><span class="nw">${sessState(st, id)} · ${esc(where)}${focus !== "—" ? " · focus " + focus : ""} · scratch ${scr}</span></span></button>`
      : `<button class="srow" data-act="open-athread" data-id="${id}"><span class="sl"><span class="mono">${id}</span> <b>${esc(t)}</b></span><span class="sm"><span class="chip ${st === "live" ? "worker" : ""}">${sessState(st, id)}</span> ${esc(where)} · focus ${focus} · $${cost.toFixed(2)} · scratch ${scr}</span></button>`).join("")}
    ${phone ? "" : `<button class="btn ghost" style="margin:8px 0;width:100%" data-act="toast" data-msg="New session A-19 started in corrald · separate from the quick thread">+ New session</button>`}
    <div class="illus" style="padding:${phone ? "6px 18px" : "4px 0"}">sessions illustrative</div>`;
  const filedBlock = (phone) => `${phone ? `<div class="ph-sec">Filed from chat<span class="n">${FILED_LIST.length}</span></div>` : `<div class="h3" style="margin-top:0">Filed from chat <span class="r">${FILED_LIST.length}</span></div>`}
    ${FILED_LIST.map(([id, t, at, src]) => `<button class="${phone ? "prow irowp fr" : "irow fr"}" data-act="open-task" data-id="${id}"><span class="mono dim">${id}</span><span class="it">${esc(t)}<span class="src">filed by assistant ${esc(src)}</span></span><span class="dim" style="font-size:12px">${at}</span></button>`).join("")}
    <div class="illus" style="padding:${phone ? "6px 18px 20px" : "4px 0"}">filed ids illustrative</div>`;

  // ---------- an assistant session's thread (same transcript view as other agents, composer docked) ----------
  const stubThread = (id) => { const s = OTHER.find((x) => x[0] === id) || OTHER[0]; return `<div class="tx qthread"><div class="tm"><span class="av o">Y</span><div><div class="hd"><b>you</b> · <span class="mono">${s[3].startsWith("pane") ? "pane" : "app:owner-desktop"}</span> · 20:58 ${stTag("sent")}</div><p>${esc(s[1])}: summarise where we are.</p></div></div>
      <div class="tm"><span class="av w">A</span><div><div class="hd"><b>assistant ${id}</b> · 20:59</div><p>${s[2] === "ended" ? "Session ended; the transcript stays readable." : "Working through it; I'll post what I find here."}</p><div class="illus">transcript illustrative</div></div></div></div>`; };
  const sessFacts = (id) => { const s = id === "A-7" ? ["A-7", "Quick thread", "live", "runs: daemon", "—", 1.12, 0] : OTHER.find((x) => x[0] === id) || OTHER[0];
    return `<div class="h3" style="margin-top:0">Session</div><dl class="kv"><dt>Id</dt><dd class="mono">${s[0]}</dd><dt>State</dt><dd>${id === "A-7" ? "live · since 14:02" : sessState(s[2], s[0])}</dd><dt>Where</dt><dd>${esc(s[3])}</dd><dt>Profile</dt><dd>${id === "A-7" || s[3] === "runs: daemon" ? `${esc(model())} <span class="dim">· switch in the composer</span>` : "pi-sol xhigh"}</dd><dt>Cost</dt><dd>$${s[5].toFixed(2)} today</dd><dt>Scratch</dt><dd>${s[6]}</dd></dl>
      <div class="dim" style="font-size:12px;margin-top:8px">The host runs until its work is done or it stops on its own (it exits, crashes, you end the session, or corrald restarts). Rolling at caps starts the next A-&lt;n&gt; with a summary.</div>`; };
  SCREENS.athread = {
    desk(S) {
      const id = S.athread || "A-7", qt = id === "A-7";
      const body = qt ? thread(false) : stubThread(id);
      const side = `<div class="pane side-col" data-scroll="at-r">${sessFacts(id)}${qt ? `<div class="h3">Filed from this thread</div><div class="ilist">${FILED_LIST.filter((f) => f[3].startsWith("A-7")).map(([fid, t]) => `<button class="irow" data-act="open-task" data-id="${fid}"><span class="mono dim">${fid}</span><span class="it">${esc(t)}</span><span></span></button>`).join("")}</div>` : ""}</div>`;
      return `${connBanner()}<div class="pagehead"><button class="btn quiet" data-act="goto" data-to="agents">${ic("back")}Agents</button><h1>${qt ? "Quick thread · A-7" : esc(id + " · " + (OTHER.find((x) => x[0] === id) || OTHER[0])[1])}</h1>${qt ? stateChip() : ""}<span class="sub">${qt ? "runs: daemon · one per workspace · every device" : "assistant session"}</span></div>
        <div class="cols stalefx" style="grid-template-columns:minmax(0,1fr) 300px"><div style="display:flex;flex-direction:column;min-height:0"><div class="scroll" style="flex:1" data-scroll="as-c">${body}</div>${composer(false, qt ? null : id)}</div>${side}</div>`;
    },
    phone(S) {
      const id = S.athread || "A-7", qt = id === "A-7";
      return { title: qt ? "Quick thread" : id, backLabel: "Agents", body: `<div style="padding:6px 0">${(qt ? thread(true) : stubThread(id)).replace(/class="tm"/g, 'class="tm" style="padding:8px 14px"')}</div>`, decide: composer(true, qt ? null : id), right: qt ? `<span style="padding-right:6px">${stateChip()}</span>` : "" };
    },
  };

  // ---------- phone composer sheet with the system keyboard ----------
  const keyboard = () => `<div class="ioskb" aria-hidden="true">${["qwertyuiop", "asdfghjkl", "zxcvbnm"].map((r, i) => `<div class="kr">${i === 2 ? '<i class="w">⇧</i>' : ""}${[...r].map((c) => `<i>${c}</i>`).join("")}${i === 2 ? '<i class="w">⌫</i>' : ""}</div>`).join("")}<div class="kr"><i class="w">123</i><i class="sp">space</i><i class="w go">return</i></div></div>`;
  const phoneSheet = () => `<div class="psheet-scrim" data-act="ask-close"></div><div class="psheet asksheet" role="dialog" aria-label="Ask"><div class="grab"></div>
      <div class="sh-h">Ask<button class="qlink" data-act="ask-open-thread">Quick thread · A-7 ›</button><button data-act="ask-close">Close</button></div>
      <div class="sh-b" style="padding:0 14px 8px">${composer(true)}</div>${keyboard()}</div>`;

  // ---------- desktop global hotkey window (main window closed; Windows desktop, Corral in the notification area) ----------
  function hotkeyFrame() {
    const hk = S.hk;
    const filed = `<div class="hkfiled"><span class="chip ok">filed · T-903</span><span class="ftitle">Gate: reuse receipts for unchanged crates after a base move</span><span class="dim">“we should cache gate receipts…” kept as written</span></div>`;
    const ans = `<p>T-867 is stale because the base moved: its gate ran on 5949a4b and main moved at 22:05. Corral queued a re-gate; nothing is needed from you.</p><div class="rests"><span class="dim">Rests on</span> <a class="ln2">corral show T-867</a> · <a class="ln2">corral log T-867</a></div>`;
    const val = hk === "typing" ? "why is T-867 sta" : hk === "filed" ? "board: show the PM inbox count in the title bar" : "why is T-867 stale? we should cache gate receipts so this stops happening";
    const result = hk === "typing" ? "" : `<div class="hkres col"><div class="dim" style="font-size:11.5px">assistant A-7 · ${hk === "filed" ? "filed" : "answered and filed"} · this ask $0.02</div>${hk === "filed" ? `<div class="hkfiled"><span class="chip ok">filed · T-902</span><span class="ftitle">Board: PM inbox count in the title bar</span><span class="dim">your words kept as written · duplicates checked: nothing close</span></div>` : ans + filed}<div class="crow"><span class="dim">Reply continues the quick thread</span><span class="grow"></span><button class="btn quiet" data-act="hk-open">Open in Corral</button></div></div>`;
    return `<div class="hkdesk win10 ${S.theme === "light" ? "lt" : ""}" role="application" aria-label="Windows desktop with the Corral Ask window">
      <div class="wsterm"><div class="wstabs"><span class="wstab on">owner@workstation: ~/main-workspace/corral</span><span class="wstab">+</span><span class="grow"></span><span>–</span><span>□</span><span>✕</span></div><div class="bigterm" style="font-size:12px;padding:12px 14px">$ corral ask "why is T-867 stale?"
<span class="w8">quick thread A-7 · assistant · answering</span>
T-867 is stale because the base moved: its gate ran on
candidate 5949a4b, then main moved at 22:05, so that verdict
belongs to an older candidate. Corral queued a re-gate.
<span class="w8">  rests on: corral show T-867 · corral log T-867 --since 20:40 · this ask $0.02</span>
$ corral ask "board: show the PM inbox count in the title bar"
<span class="ok">filed · T-902</span> Board: PM inbox count in the title bar · corral · your words kept
<span class="w8"># assistant cannot run (daemon up): the message is kept, corral ask returns</span>
$ corral ask "summarise tonight's stalls"
<span class="y" style="color:oklch(0.80 0.14 60)">waiting for assistant · provider quota · kept in the quick thread</span>
<span class="w8"># with corrald down, corral ask refuses and names the no-daemon path</span>
$ corral ask "what landed today?"
<span class="er" style="color:oklch(0.72 0.17 25)">corral ask: corrald is not running; nothing was sent. To keep an idea anyway: corral new --idea "&lt;text&gt;"</span>
$ <span class="cur"></span></div></div>
      <div class="hkwin" role="dialog" aria-label="Corral Ask window">
        <div class="hkhead">${ic("brand")}<b>Corral</b><span class="dim">Ask</span><span class="hkh">${U.healthDesk(S).replace('class="health"', 'class="health mini"')}</span><span class="grow"></span><span class="dim mono">${esc(U.HEALTH[S.health].reach)}</span></div>
        <div class="crow" style="padding:0 14px"><div class="seg"><button aria-pressed="true">Assistant</button><button>PM <span class="pmdot live"></span></button></div>${modelPick("down")}</div>
        <input id="ask-input" class="hkinput" data-bind="ask" value="${esc(val)}" aria-label="Ask">
        ${result}
        <div class="hkfoot"><span><span class="kbd">Ctrl Alt Space</span> open from anywhere</span><span><span class="kbd">↵</span> send</span><span><span class="kbd">Esc</span> hide</span><span class="grow"></span><span>main window closed · runs from the notification area</span></div>
      </div>
      <div class="taskbar"><span class="tbstart">${ic("now")}</span><span class="tbsearch">${ic("search")} Search</span><span class="tbicon on">${ic("term")}</span><span class="grow"></span>
        <span class="tray"><span class="trayicon" title="Corral: health dot; click opens Ask">${ic("brand")}<i class="tdot"></i></span><span>${ic("signal")}</span><span class="tclock">22:12<br>07/11/2026</span></span></div>
      <div class="hknote">${illus("Windows desktop, terminal and taskbar drawn for illustration; the Corral main window is closed, the daemon runs in WSL2")}</div></div>`;
  }

  // ---------- phone share sheet: caption + Send; lands in the quick thread ----------
  function sharePhone() {
    const page = `<div class="sharepage"><div class="urlbar">${ic("lock")} nsh.dev/elements/diff-view</div><h2>DiffView</h2><p>The DiffView component: the changes between two texts or of a patch, side by side or unified, with the words changed within lines highlighted, unchanged lines collapsed into separators that expand…</p><p class="dim">Another app (browser), not Corral.</p></div>`;
    if (S.share === "sheet") return `${page}<div class="psheet-scrim"></div><div class="psheet osshare" role="dialog" aria-label="Share"><div class="grab"></div>
        <div class="shhd"><div class="thumb">${ic("review")}</div><div><b>DiffView · Native Shell</b><div class="dim" style="font-size:13px">nsh.dev/elements/diff-view</div></div></div>
        <div class="apps">${[["AirDrop", ""], ["Messages", ""], ["Mail", ""], ["Corral", "corral"], ["Notes", ""]].map(([n, k]) => `<button class="appi2 ${k}" ${k ? 'data-act="share" data-v="compose"' : ""}><span class="ai">${k ? ic("brand") : ""}</span>${n}</button>`).join("")}</div>
        <div class="shacts">${["Copy", "Add to Reading List", "Add Bookmark"].map((a) => `<div>${a}</div>`).join("")}</div><div class="illus" style="padding:4px 16px">system share sheet, drawn for illustration</div></div>`;
    // The extension only writes an outbox item; the app delivers it when next opened or refreshed.
    const outbox = S.share === "saved", delivered = S.share === "delivered";
    const status = outbox ? `<div class="notice warn" style="margin-bottom:10px"><span><span class="chip held">saved on this phone</span> sends when Corral connects${off() ? " (offline now)" : ""}. <button class="btn quiet" data-act="share" data-v="delivered">Open Corral</button></span></div>`
      : delivered ? `<div style="margin-bottom:10px"><div class="dim" style="font-size:12.5px;margin-bottom:6px">In the quick thread · no caption, so the assistant filed it:</div><div class="filed"><div class="ideahd"><span class="chip ok">filed · T-905</span><span class="dim">Idea · project corral</span><span class="chip">${ic("review")} link attached</span></div><div class="dim" style="font-size:11.5px;margin-top:4px">filed by assistant A-7 from your share</div><dl class="kv fk"><dt>Title</dt><dd><b>Review: evaluate NSH DiffView annotations for inline comments</b></dd><dt>Labels</dt><dd><span class="chip">review</span> <span class="chip">nsh</span> <span class="dim">· related</span> ${task("T-874")}</dd></dl><div class="sugbtns"><button class="btn ghost">Edit</button><button class="btn ghost">Merge into T-…</button><button class="btn quiet danger-t" data-act="confirm" data-kind="discard" data-n="T-905">Discard</button></div></div><button class="btn quiet" data-act="share" data-v="" style="margin-top:6px">Done</button></div>` : "";
    return `${page}<div class="psheet-scrim"></div><div class="psheet asksheet" role="dialog" aria-label="Share to Corral"><div class="grab"></div>
      <div class="sh-h"><button data-act="share" data-v="">Cancel</button><span style="flex:1;text-align:center">Share to Corral</span><button data-act="share" data-v="saved" style="margin-left:0">${outbox || delivered ? "" : "Send"}</button></div>
      <div class="sh-b" style="padding:0 16px 10px">
        ${status}
        ${delivered ? "" : `<div class="attcard">${ic("review")}<span><b>DiffView · Native Shell</b><span class="dim mono" style="display:block;font-size:12px">https://nsh.dev/elements/diff-view</span></span><button aria-label="Remove attachment">${ic("close")}</button></div>`}
        ${outbox || delivered ? "" : `<textarea id="ask-input" data-bind="ask" rows="3" placeholder="Add a caption (optional)" style="width:100%;margin-top:10px"></textarea>
        <div class="dim" style="font-size:12.5px">Lands in the quick thread · with no caption the assistant files it · <span class="mono">via=app:owner-iphone</span></div>`}
      </div>${outbox || delivered ? "" : keyboard()}</div>`;
  }

  // ---------- actions ----------
  let timer;
  function stream() {
    clearInterval(timer);
    if (S.asst !== "answering") return;
    S.stream = 0.1;
    timer = setInterval(() => { S.stream = Math.min(1, S.stream + 0.12); render(); if (S.stream >= 1) clearInterval(timer); }, 260);
  }
  Object.assign(ACTIONS, {
    "ask-open": (d) => { S.ask.open = true; if (d && d.chip) S.ask.chip = d.chip; render(); },
    "ask-close": () => { S.ask.open = false; S.ask.chip = null; S.ask.modelMenu = false; render(); },
    "ask-target": (d) => { S.ask.target = d.v; S.ask.modelMenu = false; render(); },
    "ask-model-menu": () => { S.ask.modelMenu = !S.ask.modelMenu; render(); },
    "ask-model": (d) => { S.ask.model = d.v; S.ask.modelMenu = false; toast(`Model: ${d.v} · applies to your next message · conversation kept`); },
    "ask-unchip": () => { S.ask.chip = null; render(); },
    "ask-full": () => { S.ask.open = false; S.athread = "A-7"; nav("athread"); },
    "ask-open-thread": () => { S.ask.open = false; S.athread = "A-7"; if (S.frame === "phone") { S.ptab = "agents"; S.pstack = [{ page: "athread" }]; render(); } else nav("athread"); },
    "open-athread": (d) => { S.ask.open = false; S.athread = d.id; if (S.frame === "phone") { S.ptab = "agents"; S.pstack = [{ page: "athread" }]; render(); } else nav("athread"); },
    // Every message appears in the thread at once, before any model work.
    "ask-send": () => {
      const a = S.ask, text = ((document.getElementById("ask-input") || {}).value || a.text).trim();
      if (!text) return;
      const onSession = (S.frame === "phone" ? (S.pstack[S.pstack.length - 1] || {}).page === "athread" : S.screen === "athread") && S.athread && S.athread !== "A-7";
      if (onSession) { a.text = ""; return toast(`Sent to ${S.athread} · its reply is not simulated in the mock`); }
      S.sent = S.sent || [];
      const pmT = a.target === "pm";
      const state = off() ? "queued on this device" : pmT ? (U.HEALTH[S.health].p[0] === "live" ? "delivered" : "queued for PM") : WAIT[S.asst] ? `waiting for assistant · ${WAIT[S.asst]}` : "sent";
      S.sent.push({ text, dev: dev(), state, pm: pmT });
      a.text = "";
      if (S.frame === "phone") { S.ask.open = false; S.athread = "A-7"; if (!S.pstack.length || S.pstack[S.pstack.length - 1].page !== "athread") { S.ptab = "agents"; S.pstack = [{ page: "athread" }]; } }
      else if (S.screen !== "athread") S.ask.open = true;
      render();
    },
    asst: (d) => { S.asst = d.v; S.savedNow = false; S.withdrawn = false; if (d.v === "answering") stream(); else clearInterval(timer); render(); },
    // Escape hatch on a waiting message: file it as written, no model.
    "file-now": () => { S.savedNow = true; (S.sent || []).forEach((m) => { if (/^(waiting|queued on)/.test(m.state)) m.state = "saved · T-904"; }); toast("Filed by you, as written · no model ran"); },
    withdraw: () => { S.withdrawn = true; (S.sent || []).forEach((m) => { if (/^(waiting|queued on)/.test(m.state)) m.state = "withdrawn"; }); toast("Withdrawn · nothing filed or run"); },
    edit: (d) => { S.edit = d.v || null; render(); },
    "hk-open": () => { S.frame = "desktop"; S.screen = "athread"; S.athread = "A-7"; render(); },
    share: (d) => { S.share = d.v || null; render(); },
  });
  window.ASKUI = {
    drawer, phoneSheet, hotkeyFrame, sharePhone, stream, qtTile, qtRow, sessionsBlock, filedBlock,
    key(e) {
      if (e.key === "Escape") { e.preventDefault(); if (S.ask.modelMenu) { S.ask.modelMenu = false; return render(); } return ACTIONS["ask-close"](); }
      // Ctrl ⇧ M cycles the model without leaving the field.
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "m") { e.preventDefault(); const i = MODELS.findIndex((m) => m[0] === model()); return ACTIONS["ask-model"]({ v: MODELS[(i + 1) % MODELS.length][0] }); }
      if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); return ACTIONS["ask-send"](); }
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "p") { e.preventDefault(); S.ask.target = S.ask.target === "pm" ? "assistant" : "pm"; return render(); }
    },
    // Inbox: the assistant's pending prompts count in Needs you.
    inboxItems: () => [
      { k: "perm", lead: "Assistant A-7 asks", id: "write permission", title: "Write ~/main-workspace/notes/corral-log.md (+6 lines)?", meta: "Allow once · 1 hour · session · Deny", age: "7m" },
      { k: "conf", lead: "Assistant A-7 waits", id: "corral mode pause", title: "Confirm on this device: pause autopilot for tonight", meta: "confirm on this device · mode", age: "10m" },
    ],
    inboxPane: (k) => `<div class="qhead"><div class="meta"><span class="chip you">needs you</span><span>from the quick thread · A-7 · you were not in the thread</span></div><h1>${k === "perm" ? "Assistant wants to write a file" : "Confirm corral mode pause"}</h1></div>
      <div style="padding:16px 24px;max-width:760px">${k === "perm" ? perm() + promptTimer : actCard("corral mode pause", "confirm", "A-7/g2/i19", "waiting", { why: "mode is confirm-class", kind: "mode-pause" })}
      <div class="dim" style="font-size:12px;margin-top:10px">One answer wins: answering here also settles it in the quick thread on every device.</div>
      <button class="btn quiet" data-act="ask-open-thread" style="margin-top:8px">Open the quick thread</button></div>`,
  };
})();
