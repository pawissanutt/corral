#!/usr/bin/env python3
"""Baseline how much time and how many rounds dispatched workers lose to procedure.

T-230. Read-only and standard-library only. Sources:
  * `corral list --all --json`      task kind/size, runs, profile, harness, model, usage
  * <root>/.corral/events.jsonl     dispatch, gate, done, send-back, stale and idle events
  * <root>/.corral/artifacts/**/*changes-round-*.md   reviewer send-back notes
  * harness session transcripts     omo (and omp) worker sessions, matched per run by
                                    worktree directory and run time window (the
                                    discovery rule of crates/corral-ops/src/usage.rs)

Usage:
  scripts/measure-procedure-friction.py --root /path/containing/.corral \
      --json out.json --md out.md [--since 2026-10-15T00:00:00Z] [--until ...]

Heuristic classification is deliberate and every classified item is retained in the
JSON (`items`) so a sample can be audited. See the T-230 report for limitations.
"""
from __future__ import annotations

import argparse
import collections
import datetime as dt
import glob
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
from typing import Any

CAP_GAP_S = 15 * 60


def ts(s):
    if not s:
        return None
    s = s.replace("Z", "+00:00")
    m = re.match(r"(.*?\.\d{1,6})\d*(\+.*)$", s)
    if m:
        s = m.group(1) + m.group(2)
    try:
        return dt.datetime.fromisoformat(s)
    except ValueError:
        return None


def secs(a, b):
    return None if a is None or b is None else (b - a).total_seconds()


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def mean(xs):
    xs = [x for x in xs if x is not None]
    return sum(xs) / len(xs) if xs else None



def load_tasks(corral, root):
    out = subprocess.run(
        [corral, "list", "--all", "--json", "--root", root],
        capture_output=True, text=True, stdin=subprocess.DEVNULL, check=True,
    ).stdout
    return json.loads(out)


def corral_verbs(corral):
    try:
        out = subprocess.run([corral, "--help"], capture_output=True, text=True,
                             stdin=subprocess.DEVNULL, check=True).stdout
    except Exception:
        return set()
    verbs, on = set(), False
    for line in out.splitlines():
        if line.startswith("Commands:"):
            on = True
        elif on and re.match(r"^  [a-z][a-z-]*\s", line):
            verbs.add(line.split()[0])
        elif on and line and not line.startswith(" "):
            break
    return verbs


_ANC = {}


def is_ancestor(repo, ancestor, descendant):
    key = (repo, ancestor, descendant)
    if key not in _ANC:
        rc = subprocess.run(["git", "-C", repo, "merge-base", "--is-ancestor", ancestor, descendant],
                            capture_output=True, stdin=subprocess.DEVNULL).returncode
        _ANC[key] = True if rc == 0 else False if rc == 1 else None
    return _ANC[key]


def load_events(path):
    ev = []
    with open(path, errors="replace") as fh:
        for line in fh:
            try:
                e = json.loads(line)
            except ValueError:
                continue
            e["_t"] = ts(e.get("ts"))
            ev.append(e)
    return ev



def build_runs(tasks, since, until):
    runs = {}
    for t in tasks:
        rs = sorted(t.get("runs", []), key=lambda r: r.get("started") or "")
        for i, r in enumerate(rs):
            start = ts(r.get("started"))
            if start is None or (since and start < since) or (until and start > until):
                continue
            nxt = ts(rs[i + 1].get("started")) if i + 1 < len(rs) else None
            end = ts(r.get("ended"))
            runs[(t["id"], r["id"])] = {
                "task": t["id"], "run": r["id"], "title": t.get("title"),
                "kind": t.get("kind") or "unclassified", "size": t.get("size") or "unsized",
                "profile": r.get("agent") or "unknown", "harness": r.get("harness"),
                "model": r.get("model"), "effort": r.get("effort"),
                "worktree": r.get("worktree"), "project": t.get("project"), "outcome": r.get("outcome") or "open",
                "start": start, "end": end, "next_start": nxt,
                "window_end": end or nxt,
                "usage": r.get("usage") or {},
                "ev": collections.defaultdict(list),
            }
    return runs


def attribute_events(runs, events):
    by_task = collections.defaultdict(list)
    for (tid, _), r in runs.items():
        by_task[tid].append(r)
    for rs in by_task.values():
        rs.sort(key=lambda r: r["start"])
    for e in events:
        tid, t = e.get("task"), e["_t"]
        if tid not in by_task or t is None:
            continue
        rid = (e.get("detail") or {}).get("run")
        target = None
        if isinstance(rid, str) and (tid, rid) in runs:
            target = runs[(tid, rid)]
        else:
            for r in by_task[tid]:
                if r["start"] - dt.timedelta(seconds=5) <= t and (r["next_start"] is None or t < r["next_start"]):
                    target = r
        if target is not None:
            target["ev"][e["kind"]].append(e)


PROC_TITLE = re.compile(
    r"\b(not verified|verified line|completion summary|summary (omits|lacks|is missing|claims)|attach\w* (the )?(gate|log|evidence|report)"
    r"|(missing|unattached|absent|no) (gate|evidence|artifact|log|checkpoint|completion)\b|checkpoint (is |was )?(missing|stale)"
    r"|gate (log|result|receipt|identity) (is |was )?(missing|stale|absent)|not rebased|rebase before)", re.I)
HEADING = re.compile(r"^#{3,4}\s+(?:\d+\.\s*)?(?:P[0-3][:.\s-]*)?(.*\S)\s*$")


def reviewer_findings(artifacts_root, text):
    m = re.search(r"Details: (\S+?\.md)", text) or re.search(r"(/\S+changes-round-\d+\.md)", text)
    path = m.group(1) if m else None
    if path and not os.path.isfile(path):
        path = None
    if not path:
        return None, None
    body = open(path, errors="replace").read()
    sec = re.split(r"^## ", body, flags=re.M)
    heads = []
    for s in sec:
        if s.lower().startswith("required change"):
            for line in s.splitlines()[1:]:
                h = HEADING.match(line)
                if h:
                    heads.append(h.group(1))
    return path, heads


# Remediation components of an owner/reviewer send-back, found clause by clause in the full text.
REBASE_CL = re.compile(r"(?<![\w-])rebas(?:e|ing)\b|\bresolve (?:the )?conflicts\b|\bkeep both sides\b|\bbehind main\b"
                       r"|\bmain (?:has |had )?(?:moved|advanced)\b|\bstale result\b|stale, not a code", re.I)
REBASE_DONE = re.compile(r"(?:clean|successful|completed) rebase|\bthe rebase (?:left|introduced|caused|produced|dropped)\b|already rebased|rebased (?:cleanly|successfully)|candidate (?:was |is )?rebased", re.I)
CONFLICT_CL = re.compile(r"\bconflict(?:s|ed|ing)?\b|\bkeep both sides\b", re.I)
# Clauses that describe what landed on main; the files they name are conflict context, not a code ask.
LANDED_CL = re.compile(r"\bmain (?:gained|is now|moved)\b|\bT-\d+ [0-9a-f]{7,}\b|\bsince (?:your base|then)\b|\blanded\b|\btouching\b|\bon top of\b", re.I)
MAIN_NOT_MOVED = re.compile(r"\bmain (?:has not|hasn't|did not|didn't) (?:moved|advanced)\b|\bmain is unchanged\b", re.I)
MAIN_MOVED = re.compile(r"\bmain (?:has |had )?(?:moved|advanced|gained)\b|\bmain is now\b|\badvanced during\b"
                        r"|\blanded (?:on main\b|since\b)|\bsince then\b[^.]*\blanded\b|\b(?:after|since) your base\b"
                        r"|\b[1-9]\d* missing commits?\b", re.I)
MAIN_SHA = re.compile(r"(?:observed\s+|\bmain(?: is now| at)?\s*\(?)([0-9a-f]{7,40})\b")
DETAILS_PATH = re.compile(r"Details: (\S+?\.md)")
CONFIG_CL = re.compile(r"inputs_digest|\bconfig(?:uration)? (?:changed|change)\b|\bpolicy (?:changed|change)\b"
                       r"|\bconfigured gate now\b|\bfixed corral gate\b|\bprivate-fixture gate\b", re.I)
EVIDENCE_CL = re.compile(r"\bevidence\b|\bisolated\b|\bscreenshots?\b|\blive[- ]checks?\b|\bstress run\b|\bproof\b|\breceipt\b", re.I)
EVIDENCE_ONLY = re.compile(r"evidence only|no production rewrite", re.I)
CODE_CL = re.compile(r"\bP[0-3]\b|\bfindings?\b|\breproduced\b|\bregressions?\b|\bdefects?\b|duplicate key"
                     r"|[\w/.-]+\.(?:rs|ts|tsx|toml|py|mjs):\d+", re.I)
CODE_PATH = re.compile(r"[\w/-]+\.(?:rs|ts|tsx|toml|py|mjs)\b", re.I)
REVIEW_ASK = re.compile(r"\b(?:review|reviewer|re-review)\b[^.]{0,80}\brequested changes\b"
                        r"|\brequired changes?\b[^.]{0,60}\breview\b", re.I)
CLAUSE_SPLIT = re.compile(r"(?<=[.;])\s+|\s+(?=\d\)\s)")
SHA_TOKEN = re.compile(r"\b[0-9a-f]{7,40}\b")


def is_routine_tail(clause):
    """The boilerplate closing instruction (`fix, rebase, run the gate, then done`), not a rebase request."""
    return bool(len(clause) <= 100 and re.search(r"\brebase\b", clause, re.I) and re.search(r"\bdone\b", clause, re.I)
                and re.search(r"\b(?:run|gate|checks)\b", clause, re.I)
                and not SHA_TOKEN.search(clause) and not CONFLICT_CL.search(clause))


def remediation_components(text):
    """Which kinds of work the whole send-back asks for: rebase, evidence, config, code."""
    clauses = [c for c in CLAUSE_SPLIT.split(text.strip()) if c.strip()]
    routine = [c for c in clauses if is_routine_tail(c)]
    clauses = [c for c in clauses if c not in routine]
    comps = set()
    for cl in clauses:
        if REBASE_CL.search(cl) and not REBASE_DONE.search(cl):
            comps.add("rebase")
        if EVIDENCE_CL.search(cl):
            comps.add("evidence")
        if CONFIG_CL.search(cl):
            comps.add("config")
        if not CONFLICT_CL.search(cl) and (CODE_CL.search(cl) or (CODE_PATH.search(cl) and not LANDED_CL.search(cl))):
            comps.add("code")
    if REVIEW_ASK.search(text) and not EVIDENCE_ONLY.search(text):
        comps.add("code")
    if routine and not comps:
        comps.add("rebase")  # the closing instruction is all that was asked
    return comps, bool(routine)


REBASE_KINDS = {"submitted_stale": "rebase_submitted_stale", "main_moved_after_submit": "rebase_main_moved_after_submit",
                "not_established": "rebase_cause_not_established", "unknown": "rebase_unknown"}


def main_advanced_in_git(repo, base, shas):
    """True when a main sha named in the send-back is a strict descendant of the submission's integration base."""
    if not base:
        return False
    return any(not base.startswith(sha) and not sha.startswith(base) and is_ancestor(repo, base, sha) is True
               for sha in shas)


def rebase_cause(stale, main_moved_stated):
    """Why a rebase was needed: a stale submission, a stated main advance, or not established."""
    if stale is True:
        return "submitted_stale"
    if stale is False:
        return "main_moved_after_submit" if main_moved_stated else "not_established"
    return "unknown"
ESC_OWNER = re.compile(r"owner (?:integration|decision|resolution)|integration decision|cutover|sole-executor|baseline", re.I)
ESC_EVIDENCE = re.compile(r"\b(?:gate|isolation|herdr|launch|evidence|proof|receipt|attest\w*)\b", re.I)
ESC_TAG = re.compile(r"\[?P[0-3]\]?[:\s]")
ESC_FILE_REF = re.compile(r"[\w./-]+\.(?:rs|ts|tsx|mjs|py|toml|md):\d+")
ESC_SECTION = re.compile(r"^## [^\n]*finding[^\n]*\n(?:[ \t]*\n)*(?:### ([^\n]+)\n)?([\s\S]{0,600})", re.I | re.M)


def classify_escalation(notes):
    text = notes if isinstance(notes, str) else ""
    sec = ESC_SECTION.search(text)
    if not sec:
        return {"kind": "unknown", "heading": ""}
    title = re.search(r"^## ([^\n]+)", sec.group(0))
    heading = sec.group(1) or (title.group(1) if title else "")
    code = bool(ESC_TAG.search(heading) or ESC_FILE_REF.search(sec.group(2)))
    evidence = bool(ESC_EVIDENCE.search(heading))
    if ESC_OWNER.search(heading):
        kind = "owner_decision"
    elif code and evidence:
        kind = "mixed"
    elif code:
        kind = "code_defect"
    elif evidence:
        kind = "procedural_evidence"
    else:
        kind = "unknown"
    return {"kind": kind, "heading": heading[:140]}


def submission_stale(repo, done_event):
    d = done_event.get("detail") or {}
    head, base = (d.get("identity") or {}).get("head"), d.get("integration_base")
    if not (head and base):
        return None
    anc = is_ancestor(repo, base, head)
    return None if anc is None else not anc


ATTEMPT_ID = re.compile(r"\b(\d{3,}-\d{15,})\b")


DETAILS_HEAD_CHARS = 1500


def named_attempts(text):
    """Gate attempt ids a send-back names: in its own text and in the head of its linked Details file,
    whose first lines give the failed gate's log path (`.../T-N-R1-<attempt>.log`)."""
    ids = set(ATTEMPT_ID.findall(text))
    details = DETAILS_PATH.search(text)
    if details and os.path.isfile(details.group(1)):
        with open(details.group(1), errors="replace") as f:
            ids.update(ATTEMPT_ID.findall(f.read(DETAILS_HEAD_CHARS)))
    return ids


def resolve_gate(gates, sendback, sha, ids):
    """The gate result a `gate failed at <sha>` send-back concerns, and how it was chosen.

    Only results at that sha that precede the send-back qualify. When the send-back names attempt ids,
    only the latest qualifying result of a named attempt is used; if none exists the send-back stays
    unresolved (`named_missing`) and never falls back to another attempt. With no named id, the latest
    qualifying result is used (`latest`) when it is a failure."""
    prior = sorted((g for g in gates if str((g.get("detail") or {}).get("sha", "")).startswith(sha)
                    and g["_t"] <= sendback["_t"]), key=lambda g: g["_t"], reverse=True)
    if ids:
        named = next((g for g in prior if ((g.get("detail") or {}).get("identity") or {}).get("attempt") in ids), None)
        return named, "named" if named else "named_missing"
    latest = prior[0] if prior else None
    return (latest if latest and (latest.get("detail") or {}).get("result") == "fail" else None), "latest"


def classify_sendback(e, gates_by_task, artifacts_root):
    d = e.get("detail") or {}
    text = d.get("text") or ""
    head = text[:260]
    c = {"task": e["task"], "ts": e.get("ts"), "by": d.get("by"), "kind": "other", "procedural": False,
         "evidence": head[:160]}
    low = head.lower()
    gm = re.search(r"gate failed at ([0-9a-f]{7,40})", head)
    if gm:
        g, basis = resolve_gate(gates_by_task.get(e["task"], []), e, gm.group(1), named_attempts(text))
        c["gate_basis"] = basis
        cmd = ((g or {}).get("detail") or {}).get("failed_cmd") or ""
        if cmd.startswith("stale_result"):
            c.update(kind="stale_gate", procedural=True)
        else:
            c.update(kind="gate_failed_unclassified", procedural=False, cmd=cmd[:80])
    else:
        heads = None
        if "reviewer" in low or "independent review" in low or "independent re-review" in low:
            heads = reviewer_findings(artifacts_root, text)[1]
        if heads:
            proc = [h for h in heads if PROC_TITLE.search(h)]
            comps = ({"evidence"} if proc else set()) | ({"code"} if len(proc) < len(heads) else set())
            c.update(findings=heads, procedural_findings=proc)
        else:
            comps = remediation_components(text)[0]
        c["components"] = sorted(comps)
        details = DETAILS_PATH.search(text)
        details_text = open(details.group(1), errors="replace").read(2000) if details and os.path.isfile(details.group(1)) else ""
        stated = bool(MAIN_MOVED.search(text) and not MAIN_NOT_MOVED.search(text))
        c["main_moved_stated"] = stated or bool(MAIN_MOVED.search(details_text))
        c["main_advance_basis"] = "stated" if stated else "details" if c["main_moved_stated"] else None
        c["main_shas"] = MAIN_SHA.findall(text)[:4]
        if low.startswith("owner note"):
            c.update(kind="owner_note")  # context for the run, not a defect or procedure request
        elif "code" in comps:
            procedure = bool(comps & {"rebase", "evidence", "config"})
            c.update(kind="mixed_code_and_procedure" if procedure else "code_defect", procedural=False, mixed=procedure)
        elif "rebase" in comps and "config" in comps and MAIN_NOT_MOVED.search(text):
            c.update(kind="config_policy_stale", procedural=False)
        elif "rebase" in comps:
            c.update(kind="rebase_and_evidence_request" if "evidence" in comps else "rebase_request", procedural=False)
        elif "config" in comps:
            c.update(kind="config_policy_stale", procedural=False)
        elif "evidence" in comps:
            c.update(kind="evidence_request", procedural=True)
    return c


NOISE_ERR = re.compile(r"^error(\[E\d+\])?: (test failed|could not compile|aborting|build failed)", re.I)
CLAP = re.compile(r"^error: (unexpected argument|unrecognized subcommand|the following required arguments"
                  r"|invalid value|a value is required|unexpected value|invalid subcommand)", re.I | re.M)
REAL_CORRAL = r"(?:(?<=/bin/)|(?<![\w./-]))corral"
# An executed installed binary, shell form (`corral x`) or argv form (`['/home/u/.cargo/bin/corral','x']`), valid verb or not.
EXEC_CORRAL = re.compile(r"(?:(?<=/bin/)|(?<![\w./$-]))(?<!\$ )corral(?![\w./-])(?:\s+[\w-]|['\"]\s*,\s*['\"])")
READER_CMD = re.compile(r"^\s*(?:rg|grep|sed|cat|head|tail|awk|git\s+(?:grep|log|show|diff))\b")
BUILD_CMD = re.compile(r"\bcargo\s+(?:test|nextest|run)\b")
GATE_OUT = re.compile(r"==> cargo fmt --all -- --check|FAILED: (?:cargo|800-SLOC|CORRAL_HERDR|tests invoked)")
DONE_CALL = re.compile(r"\bdone\s+['\"]?T-\d+")
REBASE = re.compile(r"\bgit\s+(?:-C\s+\S+\s+)?(?:rebase|merge|pull)\b")
CORRAL_ALIAS = r"(?:corral|\$\{?C(?:ORRAL)?\}?|\{C\}|\$\{corral\}|\$\{CORRAL\}|\$\{bin\}|\$\{CORRAL_BIN\})"


MODE_HELP = "looked up `corral --help` / `<verb> --help`"
MODE_VERIFIED = "missing Verified / Not verified line"


def invalid_invocation(text, result, context=""):
    """The clap usage error of an executed installed `corral`; source mentions and dev builds are excluded.

    `context` holds recent calls that ran the binary, for a background session's error read in a later call."""
    clap = CLAP.search(result)
    if not clap or "Usage: corral" not in result or BUILD_CMD.search(text) or READER_CMD.match(text):
        return None
    if not EXEC_CORRAL.search(text):
        return None
    named = re.search(r"'([^']+)'", result[clap.start():].split("\n", 1)[0])
    return clap if named is None or named.group(1) in text or named.group(1) in context else None


def invalid_label(msg):
    return "invalid corral invocation: " + re.sub(r"'[^']*'", "'…'", msg)


def reject_label(reason):
    key = reason.split(":")[1].strip() if reason.startswith("error:") and ":" in reason[6:] else reason
    return "`corral done` rejected: " + re.sub(r"\s+", " ", key)[:60]


REJECT_KEYS = ("summary_section_missing", "worktree_dirty", "checkpoint_required", "checkpoint_stale",
               "gate_stale", "gate_incomplete", "missing_gate_identity", "missing_artifact", "wrong_attempt",
               "integration_base_not_ancestor")
CODE_LINE = re.compile(r"format!|\.push\(|bail!|assert|^\s*\d+[-:]|^\s*[+-]\s")


def texts(obj):
    if isinstance(obj, str):
        yield obj
    elif isinstance(obj, dict):
        for v in obj.values():
            yield from texts(v)
    elif isinstance(obj, list):
        for v in obj:
            yield from texts(v)


def result_text(m):
    c = m.get("content")
    if isinstance(c, str):
        return c
    if isinstance(c, list):
        return "\n".join(x.get("text", "") for x in c if isinstance(x, dict))
    return ""


def omo_dir():
    for k in ("OMO_CODING_AGENT_DIR", "SENPI_CODING_AGENT_DIR"):
        if os.environ.get(k):
            return os.environ[k]
    return os.path.expanduser("~/.omo/agent")


def session_dirs(run, home):
    wt = run["worktree"] or ""
    cands = [("omo", os.path.join(omo_dir(), "sessions", "--" + wt.strip("/").replace("/", "-") + "--"))]
    rel = wt[len(home) + 1:] if wt.startswith(home + "/") else None
    if rel:
        cands.append(("omp", os.path.join(home, ".omp/agent/sessions", "-" + rel.replace("/", "-"))))
    return [(h, d) for h, d in cands if os.path.isdir(d)]


def worker_sessions(run, home):
    out = []
    for harness, d in session_dirs(run, home):
        for f in sorted(glob.glob(os.path.join(d, "*.jsonl"))):
            name = None
            with open(f, errors="replace") as fh:
                for i, line in enumerate(fh):
                    if i > 8:
                        break
                    try:
                        r = json.loads(line)
                    except ValueError:
                        continue
                    if r.get("type") == "session_info":
                        name = r.get("name")
            if name and re.search(r"\breview\b\s*$", name):
                continue
            out.append((harness, f))
    return out


def scan_transcripts(run, home, verb_re, real_re, help_re):
    sessions = worker_sessions(run, home)
    if not sessions:
        return None
    lo = run["start"] - dt.timedelta(seconds=30)
    hi = (run["window_end"] + dt.timedelta(seconds=120)) if run["window_end"] else None
    calls = []
    index = {}
    for harness, f in sessions:
        with open(f, errors="replace") as fh:
            for line in fh:
                try:
                    r = json.loads(line)
                except ValueError:
                    continue
                m = r.get("message") or {}
                t = ts(r.get("timestamp"))
                if t is None or t < lo or (hi and t > hi):
                    continue
                if m.get("role") == "assistant" and isinstance(m.get("content"), list):
                    for c in m["content"]:
                        if isinstance(c, dict) and c.get("type") == "toolCall":
                            text = "\n".join(texts(c.get("arguments")))
                            rec = {"t": t, "name": c.get("name"), "text": text, "result": ""}
                            index[c.get("id")] = rec
                            calls.append(rec)
                elif m.get("role") == "toolResult":
                    pending = index.get(m.get("toolCallId"))
                    if pending is not None:
                        pending["result"] += result_text(m)
    calls.sort(key=lambda c: c["t"])
    facts: dict[str, Any] = {"calls": len(calls), "corral_calls": 0, "help": [], "invalid": [], "done": [], "gate_runs": [], "rounds": [],
             "rejects": [], "rebase_calls": [], "verbs": collections.Counter(), "first_call": calls[0]["t"] if calls else None}
    for i, c in enumerate(calls):
        runs_gate_sh = "scripts/gate.sh" in c["text"] and ("corral_test_herdr_start" in c["text"] or "</dev/null" in c["text"])
        manual_gate = "cargo clippy" in c["text"] and "cargo test" in c["text"] and "-- --check" in c["text"]
        if GATE_OUT.search(c["result"]) or (c["name"] in ("bash", "eval", "bash_input") and (runs_gate_sh or manual_gate) and c["result"]):
            facts["gate_runs"].append(c["t"])
        if c["name"] not in ("bash", "eval", "bash_input"):
            continue
        text = c["text"]
        if "corral" not in text.lower() and "${C}" not in text and "{C}" not in text:
            continue
        nxt = calls[i + 1]["t"] if i + 1 < len(calls) else None
        gap = min(secs(c["t"], nxt) or 0, CAP_GAP_S) if nxt else None
        invs = [m.group(1) for m in verb_re.finditer(text)]
        if "gate" in [m.group(1) for m in real_re.finditer(text)] and not help_re.search(text):
            facts["gate_runs"].append(c["t"])
        helps = help_re.findall(text)
        for v in invs:
            facts["verbs"][v] += 1
        if invs or helps:
            facts["corral_calls"] += 1
        signals: collections.Counter[str] = collections.Counter()
        for h in helps:
            facts["help"].append({"t": c["t"], "what": " ".join(h.split())[:50]})
            signals[MODE_HELP] += 1
        res = c["result"]
        real = [m.group(1) for m in real_re.finditer(text)]
        recent = "\n".join(p["text"] for p in calls[max(0, i - 5):i]
                           if p["name"] in ("bash", "eval", "bash_input") and EXEC_CORRAL.search(p["text"]))
        clap = invalid_invocation(text, res, recent)
        if clap:
            if not (invs or helps):
                facts["corral_calls"] += 1
            first = clap.group(0)
            facts["invalid"].append({"t": c["t"], "msg": first[:80], "verbs": sorted(set(real))[:3]})
            signals[invalid_label(first[:80])] += 1
        if REBASE.search(text):
            facts["rebase_calls"].append(c["t"])
        if "done" in invs and DONE_CALL.search(text):
            ok = "-> Review" in res
            lines = [l.strip() for l in res.splitlines()
                     if re.match(r"^\s*error(\[E\d+\])?:", l) and not NOISE_ERR.match(l.strip())]
            rec: dict[str, Any] = {"t": c["t"], "ok": ok, "reason": None}
            if not ok and lines:
                rec["reason"] = re.sub(r"[0-9a-f]{7,40}", "<sha>", re.sub(r"T-\d+", "T-N", lines[0]))[:110]
                signals[reject_label(rec["reason"])] += 1
            facts["done"].append(rec)
        if real:
            for key in REJECT_KEYS:
                if any(key in l and len(l) < 240 and not CODE_LINE.search(l) for l in res.splitlines()):
                    facts["rejects"].append({"t": c["t"], "key": key})
                    if key == "summary_section_missing":
                        signals[MODE_VERIFIED] += 1
        if signals:
            facts["rounds"].append({"t": c["t"], "gap": gap or 0, "signals": dict(signals)})
    return facts



GATE_ERA = [None]


def run_metrics(run, ts_facts, gates_by_task, artifacts_root):
    ev = run["ev"]
    disp = next((e["_t"] for e in sorted(ev.get("dispatched", []), key=lambda e: e["_t"])), None) or run["start"]
    gstart = sorted(e["_t"] for e in ev.get("gate_started", []))
    gres = [e for e in ev.get("gate", [])]
    dones = sorted(e["_t"] for e in ev.get("done", []))
    sb_raw = sorted(ev.get("changes_requested", []), key=lambda e: e["_t"])
    sb: list[dict[str, Any]] = [classify_sendback(e, gates_by_task, artifacts_root) | {"_t": e["_t"]} for e in sb_raw]
    for s in sb:
        nxt = next((d for d in dones if d > s["_t"]), None)
        s["latency_s"] = secs(s["_t"], nxt)
    first_done = dones[0] if dones else None
    stale_gate_ev = [e for e in gres if ((e.get("detail") or {}).get("failed_cmd") or "").startswith("stale_result")]
    stale_gate_events = len(stale_gate_ev)
    stale_gate_gap_s = sum(secs(e["_t"], next((g for g in gstart if g > e["_t"]), None)) or 0 for e in stale_gate_ev)
    idle = {(e.get("detail") or {}).get("idle_since") for e in ev.get("idle_no_report", [])}
    landing = collections.Counter()
    for e in ev.get("landing_stale", []):
        reason = (e.get("detail") or {}).get("reason") or ""
        if "gate missing" in reason:
            landing["stale_gate"] += 1
        elif "semantic review missing" in reason or "review the integrated candidate" in reason:
            landing["stale_review"] += 1
        elif "review in progress" in reason:
            landing["review_in_progress"] += 1
        else:
            landing["other"] += 1
    m = {
        "dispatch": disp, "first_gate_s": secs(disp, gstart[0]) if gstart else None,
        "first_done_s": secs(disp, first_done), "gates_started": len(gstart),
        "gates_before_first_done": sum(1 for g in gstart if first_done and g < first_done),
        "gate_results": len(gres), "gate_failed": sum(1 for e in gres if (e.get("detail") or {}).get("result") == "fail"),
        "stale_gate_results": stale_gate_events, "stale_gate_gap_s": stale_gate_gap_s, "gate_invalidated": len(ev.get("gate_invalidated", [])),
        "submissions": len(dones), "sendbacks": sb,
        "idle_no_done": len([i for i in idle if i]) or (1 if ev.get("idle_no_report") else 0),
        "landing": dict(landing), "candidate_rebased": len(ev.get("candidate_rebased", [])),
        "reviewer_verdicts": len(ev.get("verdict", [])),
        "cost_usd": run["usage"].get("estimated_usd") if run["usage"].get("estimated_usd") is not None else run["usage"].get("cost_usd"),
        "tokens": sum(int(run["usage"].get(k) or 0) for k in ("input", "output", "cache_read", "cache_write")) if run["usage"] else None,
    }
    m["post_gate_era"] = bool(GATE_ERA[0] and run["start"] >= GATE_ERA[0] and run.get("project") == "corral")
    m["sendbacks_total"] = len(sb)
    m["sendbacks_procedural"] = sum(1 for s in sb if s["procedural"])
    m["sendbacks_mixed"] = sum(1 for s in sb if s.get("mixed"))
    m["proc_latency_s"] = sum(s["latency_s"] or 0 for s in sb if s["procedural"])
    repo = (run["worktree"] or "").split("/.worktrees/")[0]
    subs = sorted(ev.get("done", []), key=lambda e: e["_t"])
    sub_stale = [submission_stale(repo, e) for e in subs]
    m["submissions_stale_base"] = sum(1 for x in sub_stale if x)
    m["stale_base_evaluable"] = int(any(x is not None for x in sub_stale))
    m["first_submission_stale_base"] = sub_stale[0] if sub_stale else None
    m["first_submission_evaluable"] = int(bool(sub_stale) and sub_stale[0] is not None)
    for s_ in sb:
        idx = max((i for i, e in enumerate(subs) if e["_t"] <= s_["_t"]), default=None)
        s_["submission_index"] = idx
        s_["submission_stale_base"] = sub_stale[idx] if idx is not None else None
        if "rebase" in s_.get("components", ()):
            moved = s_.get("main_moved_stated", False)
            if not moved and idx is not None:
                moved = main_advanced_in_git(repo, (subs[idx].get("detail") or {}).get("integration_base"), s_.get("main_shas", ()))
                s_["main_advance_basis"] = "git" if moved else None
            s_["rebase_cause"] = rebase_cause(s_["submission_stale_base"], moved)
        if s_["kind"] == "rebase_request":
            s_["kind"] = REBASE_KINDS[s_["rebase_cause"]]
            s_["procedural"] = s_["rebase_cause"] == "submitted_stale"
        elif s_["kind"] == "rebase_and_evidence_request":
            s_["kind"] = "rebase_and_evidence"
            s_["procedural"] = s_["rebase_cause"] == "submitted_stale"
    m["sendbacks_procedural"] = sum(1 for s_ in sb if s_["procedural"])
    m["proc_latency_s"] = sum(s_["latency_s"] or 0 for s_ in sb if s_["procedural"])
    m["done_before_rebase"] = bool(m["first_submission_stale_base"])
    m["rebase_after_submit_runs"] = int(any(
        s_.get("rebase_cause") == "main_moved_after_submit" and s_["kind"] in ("rebase_main_moved_after_submit", "rebase_and_evidence")
        for s_ in sb))
    resolutions = sorted(ev.get("done", []) + ev.get("approved", []), key=lambda e: e["_t"])
    m["escalations"] = [
        classify_escalation((e.get("detail") or {}).get("notes")) | {
            "task": run["task"], "run": run["run"], "ts": e.get("ts"),
            "latency_s": secs(e["_t"], next((x["_t"] for x in resolutions if x["_t"] > e["_t"]), None))}
        for e in sorted(ev.get("verdict", []), key=lambda e: e["_t"])
        if (e.get("detail") or {}).get("verdict") == "escalate"]
    t = ts_facts
    if t is None:
        m["transcript"] = False
    else:
        m["transcript"] = True
        m["tool_calls"] = t["calls"]
        m["corral_calls"] = t["corral_calls"]
        m["help_calls"] = len(t["help"])
        m["invalid_calls"] = len(t["invalid"])
        m["done_attempts"] = len(t["done"])
        m["done_rejected"] = sum(1 for d in t["done"] if not d["ok"] and d["reason"])
        m["done_unknown"] = sum(1 for d in t["done"] if not d["ok"] and not d["reason"])
        pre = sorted({g for g in t["gate_runs"] if first_done is None or g <= first_done})
        m["worker_gate_runs"] = len(t["gate_runs"])
        m["worker_gates_before_done"] = len(pre)
        m["first_worker_gate_s"] = secs(disp, min(pre)) if pre else None
        m["rebase_calls_before_done"] = sum(1 for r_ in t["rebase_calls"] if first_done is None or r_ <= first_done)
        m["reject_keys"] = dict(collections.Counter(r["key"] for r in t["rejects"]))
        m["friction_rounds"] = len(t["rounds"])
        m["help_rounds"] = sum(1 for rd in t["rounds"] if MODE_HELP in rd["signals"])
        m["invalid_rounds"] = sum(1 for rd in t["rounds"] if any(k.startswith("invalid corral") for k in rd["signals"]))
        m["rejected_done_rounds"] = sum(1 for rd in t["rounds"] if any(k.startswith("`corral done` rejected") for k in rd["signals"]))
        m["friction_gap_s"] = sum(rd["gap"] for rd in t["rounds"])
        m["_t"] = t
    return m


GROUPS = {"profile": lambda r: r["profile"], "harness": lambda r: r["_harness"],
          "kind": lambda r: r["kind"], "size": lambda r: r["size"]}


def mins(x):
    return None if x is None else x / 60


def agg(rs):
    ms = [r["m"] for r in rs]
    tr = [m for m in ms if m["transcript"]]
    sb = [s for m in ms for s in m["sendbacks"]]
    pr = [s for s in sb if s["procedural"]]
    ck = lambda f, xs: sum(f(x) for x in xs)
    return {
        "runs": len(rs), "runs_with_transcript": len(tr),
        "median_first_worker_gate_min": mins(med([m.get("first_worker_gate_s") for m in tr])),
        "median_first_done_min": mins(med([m["first_done_s"] for m in ms])),
        "runs_no_gate_before_done": sum(1 for m in tr if m["post_gate_era"] and m["submissions"] and not m["worker_gates_before_done"]),
        "post_gate_era_runs": sum(1 for m in tr if m["post_gate_era"] and m["submissions"]),
        "help_calls": ck(lambda m: m["help_calls"], tr), "invalid_calls": ck(lambda m: m["invalid_calls"], tr),
        "runs_with_help": sum(1 for m in tr if m["help_calls"]), "runs_with_invalid": sum(1 for m in tr if m["invalid_calls"]),
        "done_attempts": ck(lambda m: m["done_attempts"], tr), "done_rejected": ck(lambda m: m["done_rejected"], tr),
        "runs_with_rejected_done": sum(1 for m in tr if m["done_rejected"]),
        "sendbacks": len(sb), "sendbacks_procedural": len(pr),
        "sendbacks_mixed": sum(1 for s in sb if s.get("mixed")),
        "runs_with_sendback": sum(1 for m in ms if m["sendbacks"]),
        "runs_with_procedural_sendback": sum(1 for m in ms if any(s["procedural"] for s in m["sendbacks"])),
        "procedural_latency_h": sum(s["latency_s"] or 0 for s in pr) / 3600,
        "median_procedural_latency_min": (med([s["latency_s"] for s in pr]) or 0) / 60 if pr else None,
        "stale_gate_results": ck(lambda m: m["stale_gate_results"], ms),
        "done_before_rebase_runs": sum(1 for m in ms if m["done_before_rebase"]),
        "submissions_stale_base": ck(lambda m: m["submissions_stale_base"], ms),
        "stale_base_evaluable_runs": ck(lambda m: m["stale_base_evaluable"], ms),
        "rebase_after_submit_runs": ck(lambda m: m["rebase_after_submit_runs"], ms),
        "first_submission_evaluable_runs": ck(lambda m: m["first_submission_evaluable"], ms),
        "sendback_elapsed_h": sum(s["latency_s"] or 0 for s in sb) / 3600,
        "gate_sendback_basis": dict(collections.Counter(s["gate_basis"] for s in sb if s.get("gate_basis"))),
        "sendbacks_code": sum(1 for s in sb if s["kind"] == "code_defect"),
        "sendbacks_config": sum(1 for s in sb if s["kind"] == "config_policy_stale"),
        "sendbacks_rebase_unestablished": sum(1 for s in sb if s["kind"] in ("rebase_cause_not_established", "rebase_unknown")),
        "mixed_interval_h": sum(s["latency_s"] or 0 for s in sb if s.get("mixed")) / 3600,
        "idle_no_done_runs": sum(1 for m in ms if m["idle_no_done"]),
        "median_cost_usd": med([m["cost_usd"] for m in ms]),
        "friction_gap_h": ck(lambda m: m.get("friction_gap_s", 0), tr) / 3600,
        "friction_rounds": ck(lambda m: m["friction_rounds"], tr), "help_rounds": ck(lambda m: m["help_rounds"], tr),
        "invalid_rounds": ck(lambda m: m["invalid_rounds"], tr), "rejected_done_rounds": ck(lambda m: m["rejected_done_rounds"], tr),
        "escalations": sum(len(m["escalations"]) for m in ms),
        "escalations_procedural_evidence": sum(1 for m in ms for e in m["escalations"] if e["kind"] == "procedural_evidence"),
        "escalations_owner_decision": sum(1 for m in ms for e in m["escalations"] if e["kind"] == "owner_decision"),
        "escalations_code_defect": sum(1 for m in ms for e in m["escalations"] if e["kind"] == "code_defect"),
        "escalations_mixed_or_unknown": sum(1 for m in ms for e in m["escalations"] if e["kind"] in ("mixed", "unknown")),
        "run_hours_to_first_done": sum(m["first_done_s"] or 0 for m in ms) / 3600,
    }


def failure_modes(runs):
    modes: dict[str, Any] = collections.defaultdict(lambda: {"occurrences": 0, "runs": set(), "time_lost_s": 0.0, "tasks": collections.Counter()})

    def add(mode, r, n=1, lost=0.0, rounds=0, interval=0.0):
        x = modes[mode]
        x["interval_s"] = x.get("interval_s", 0.0) + (interval or 0.0)
        x["rounds"] = x.get("rounds", 0) + rounds
        x["occurrences"] += n
        x["runs"].add((r["task"], r["run"]))
        x["time_lost_s"] += lost or 0.0
        x["tasks"][r["task"]] += n

    for r in runs.values():
        m = r["m"]
        for s in m["sendbacks"]:
            if s["kind"] == "stale_gate":
                add("send-back: stale gate result (candidate/base/spec changed during gate)", r, 1, s["latency_s"])
            elif s["kind"] == "rebase_submitted_stale" or (s["kind"] == "rebase_and_evidence" and s["procedural"]):
                add("send-back: submitted on a stale base (main already moved; no rebase before `done`)", r, 1, s["latency_s"])
            elif s["kind"] in ("rebase_unknown", "rebase_cause_not_established"):
                add("send-back: rebase requested, cause not established (no submission identity, or current submission and no main advance stated)", r, 1, s["latency_s"])
            elif s["kind"] == "rebase_main_moved_after_submit":
                add("send-back: rebase after main moved while the run waited in review (queue drift)", r, 1, s["latency_s"])
            elif s["kind"] == "rebase_and_evidence":
                add("send-back: rebase plus evidence request after main moved while the run waited in review", r, 1, s["latency_s"])
            elif s["kind"] == "config_policy_stale":
                add("send-back: re-review/re-gate after a gate, config or policy change (not a main advance)", r, 1, s["latency_s"])
            elif s["kind"] == "evidence_request":
                add("send-back: evidence/summary/artifacts/checkpoint requested", r, 1, s["latency_s"])
            elif s["kind"] == "mixed_code_and_procedure":
                add("send-back mixing code fixes with rebase/evidence work (procedure share not separable; interval not charged)",
                    r, 1, 0.0, interval=s["latency_s"])
        if m["stale_gate_results"]:
            add("gate result went stale (candidate/base/spec/policy changed while the gate ran)", r, m["stale_gate_results"], m["stale_gate_gap_s"])
        if m["landing"].get("stale_gate"):
            add("landing blocked: gate missing/stale for integrated candidate", r, m["landing"]["stale_gate"])
        if m["landing"].get("stale_review"):
            add("landing blocked: semantic review missing/stale", r, m["landing"]["stale_review"])
        if m["idle_no_done"]:
            add("worker stopped without `corral done`", r, m["idle_no_done"])
        if not m["transcript"]:
            continue
        t = m["_t"]
        if m["post_gate_era"] and m["submissions"] and not m["worker_gates_before_done"]:
            add("first `corral done` with no worker-side gate run in the transcript", r, 1)
        for rd in t["rounds"]:
            share = rd["gap"] / len(rd["signals"])
            for label, n in rd["signals"].items():
                add(label, r, n, share, rounds=1)
        for e in m["escalations"]:
            if e["kind"] == "procedural_evidence":
                add("semantic escalation verdict: procedural / evidence (gate or launch-environment proof requested)", r, 1, e["latency_s"])
    out = []
    for name, x in modes.items():
        out.append({"mode": name, "occurrences": x["occurrences"], "rounds": x.get("rounds", 0), "runs": len(x["runs"]),
                    "time_lost_h": round(x["time_lost_s"] / 3600, 2),
                    "interval_h": round(x.get("interval_s", 0.0) / 3600, 2),
                    "tasks": [t for t, _ in x["tasks"].most_common(8)]})
    out.sort(key=lambda o: (-o["time_lost_h"], -o["occurrences"]))
    return out


def system_noise(events, runs_tasks):
    c = collections.Counter()
    for e in events:
        d = e.get("detail") or {}
        k = e.get("kind")
        if k == "autopilot_error":
            s = d.get("error") or ""
            c["autopilot: machine busy / waiting (system)" if "machine busy" in s or "waiting for finishing" in s
              else "autopilot: pane inventory / launch in progress (system)" if "pane inventory" in s or "launch is in progress" in s
              else "autopilot: reply not delivered" if "reply not delivered" in s else "autopilot: other"] += 1
        elif k == "dispatch_failed":
            c["dispatch_failed (pane inventory / in progress / policy)"] += 1
        elif k == "landing_stale" and "review in progress" in (d.get("reason") or ""):
            c["landing waits for review in progress (system sequencing)"] += 1
        elif k == "needs_me" and d.get("note") == "agent pane is gone":
            c["needs_me: agent pane is gone (system)"] += 1
        elif k == "review_escalated":
            c["review_escalated (reviewer closed without verdict / stale review)"] += 1
    return dict(c)



def fmt(x, nd=1):
    if x is None:
        return "-"
    return f"{x:.{nd}f}" if isinstance(x, float) else str(x)


def md_table(title, rows, cols):
    out = [f"### {title}", "", "| group | " + " | ".join(c[0] for c in cols) + " |",
           "|---|" + "|".join("---:" for _ in cols) + "|"]
    for g, a in rows:
        out.append(f"| {g} | " + " | ".join(fmt(a.get(c[1]), c[2] if len(c) > 2 else 1) for c in cols) + " |")
    return "\n".join(out) + "\n"


COLS = [("runs", "runs", 0), ("w/ transcript", "runs_with_transcript", 0),
        ("help calls", "help_calls", 0), ("help rounds", "help_rounds", 0), ("invalid calls", "invalid_calls", 0),
        ("done tries", "done_attempts", 0), ("done rejected", "done_rejected", 0),
        ("send-backs", "sendbacks", 0), ("procedural", "sendbacks_procedural", 0),
        ("proc. lost h", "procedural_latency_h", 1), ("stale-gate results", "stale_gate_results", 0),
        ("1st done on stale base", "done_before_rebase_runs", 0), ("1st done measurable", "first_submission_evaluable_runs", 0),
        ("code", "sendbacks_code", 0), ("mixed code+procedure", "sendbacks_mixed", 0),
        ("rebase after submit", "rebase_after_submit_runs", 0),
        ("idle w/o done", "idle_no_done_runs", 0), ("no gate before done", "runs_no_gate_before_done", 0),
        ("med. min to 1st worker gate", "median_first_worker_gate_min", 0), ("med. min to 1st done", "median_first_done_min", 0)]


ESC_COLS = [("runs", "runs", 0), ("escalation verdicts", "escalations", 0), ("procedural/evidence", "escalations_procedural_evidence", 0),
            ("owner decision", "escalations_owner_decision", 0), ("code defect", "escalations_code_defect", 0),
            ("mixed/unknown", "escalations_mixed_or_unknown", 0)]


def render_md(result):
    o = [f"# Procedure friction baseline (generated {result['generated']})", "",
         f"Runs measured: {result['totals']['runs']} (window {result['window']}). "
         f"Runs with a worker transcript: {result['totals']['runs_with_transcript']}.", ""]
    o.append(md_table("All runs", [("all", result["totals"])], COLS))
    for key in ("harness", "profile", "kind", "size"):
        o.append(md_table(f"By {key}", sorted(result["by_" + key].items()), COLS))
    o.append(md_table("Semantic escalation verdicts, all runs", [("all", result["totals"])], ESC_COLS))
    for key in ("harness", "profile", "kind"):
        o.append(md_table(f"Semantic escalation verdicts by {key}", sorted(result["by_" + key].items()), ESC_COLS))
    o.append("### Escalation verdicts\n\n| task | run | class | latency to resubmission or landing (min) | heading |\n|---|---|---|---:|---|")
    for e in result["escalations"]:
        o.append(f"| {e['task']} | {e['run']} | {e['kind']} | {fmt(None if e['latency_s'] is None else e['latency_s'] / 60, 0)} | {e['heading']} |")
    o.append("\n### Top procedural failure modes\n\n| mode | occurrences | tool rounds | runs | time lost (h) | uncharged mixed interval (h) | example tasks |\n|---|---:|---:|---:|---:|---:|---|")
    for f in result["failure_modes"][:20]:
        o.append(f"| {f['mode']} | {f['occurrences']} | {f['rounds']} | {f['runs']} | {f['time_lost_h']} | {f['interval_h']} | {', '.join(f['tasks'])} |")
    o.append("\n### System noise (excluded from worker friction)\n\n| signal | events |\n|---|---:|")
    for k, v in sorted(result["system_noise"].items(), key=lambda kv: -kv[1]):
        o.append(f"| {k} | {v} |")
    o.append("\n### Send-back classification\n\n| kind | send-backs | procedural | elapsed to resubmission (h) |\n|---|---:|---:|---:|")
    for k, v in sorted(result["sendback_kinds"].items(), key=lambda kv: -kv[1]["n"]):
        o.append(f"| {k} | {v['n']} | {v['procedural']} | {v['hours']:.1f} |")
    return "\n".join(o) + "\n"


def build_regexes(verbs):
    alt = "|".join(sorted(verbs, key=len, reverse=True))
    verb_re = re.compile(r"(?:(?<=/bin/)|(?<![\w./-]))" + CORRAL_ALIAS + r"(?:\s+--(?:root|project)[ =]\S+)*\s+(" + alt + r")\b")
    real_re = re.compile(REAL_CORRAL + r"(?:\s+--(?:root|project)[ =]\S+)*\s+(" + alt + r")\b")
    help_re = re.compile(r"(?:(?<=/bin/)|(?<![\w./-]))" + CORRAL_ALIAS + r"(?:\s+[a-z][a-z-]*){0,2}\s+(?:--help|-h)\b|(?:(?<=/bin/)|(?<![\w./-]))"
                         + CORRAL_ALIAS + r"\s+help\b")
    return verb_re, real_re, help_re


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", default=os.getcwd(), help="directory containing .corral/ (default: cwd)")
    ap.add_argument("--corral", default=shutil.which("corral") or os.path.expanduser("~/.cargo/bin/corral"))
    ap.add_argument("--since", help="only runs dispatched at/after this RFC3339 time")
    ap.add_argument("--until", help="only runs dispatched at/before this RFC3339 time")
    ap.add_argument("--json", dest="json_out", help="write full results (with per-item evidence) here")
    ap.add_argument("--md", dest="md_out", help="write the summary tables here (default: stdout)")
    ap.add_argument("--exclude-task", action="append", default=[], help="task id to leave out (e.g. this spike)")
    args = ap.parse_args()

    root = os.path.abspath(args.root)
    home = os.path.expanduser("~")
    events_path = os.path.join(root, ".corral", "events.jsonl")
    if not os.path.isfile(events_path):
        sys.exit(f"no {events_path}; pass --root <dir containing .corral>")
    tasks = load_tasks(args.corral, root)
    events = load_events(events_path)
    runs = build_runs(tasks, ts(args.since) if args.since else None, ts(args.until) if args.until else None)
    for x in args.exclude_task:
        for k in [k for k in runs if k[0] == x]:
            del runs[k]
    attribute_events(runs, events)
    lo, hi = (ts(args.since) if args.since else None), (ts(args.until) if args.until else None)
    window_events = [e for e in events if e["_t"] and (not lo or e["_t"] >= lo) and (not hi or e["_t"] <= hi)]
    verbs = corral_verbs(args.corral) or {"done", "gate", "artifact", "run", "show", "list", "verdict", "review"}
    verb_re, real_re, help_re = build_regexes(verbs)
    gate_starts = [e["_t"] for e in events if e.get("kind") == "gate_started" and e["_t"]]
    GATE_ERA[0] = min(gate_starts) if gate_starts else None
    gates_by_task = collections.defaultdict(list)
    for e in events:
        if e.get("kind") == "gate":
            gates_by_task[e["task"]].append(e)
    artifacts_root = os.path.join(root, ".corral", "artifacts")

    for key, r in runs.items():
        facts = scan_transcripts(r, home, verb_re, real_re, help_re)
        r["m"] = run_metrics(r, facts, gates_by_task, artifacts_root)
        r["_harness"] = r["harness"] or ("omo" if facts is not None else "unknown(legacy)")

    all_runs = list(runs.values())
    result = {
        "generated": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "window": f"{min(r['start'] for r in all_runs):%Y-%m-%d} .. {max(r['start'] for r in all_runs):%Y-%m-%d}",
        "events": len(events), "totals": agg(all_runs),
        "failure_modes": failure_modes(runs), "system_noise": system_noise(window_events, None),
    }
    for g, fn in GROUPS.items():
        buckets = collections.defaultdict(list)
        for r in all_runs:
            buckets[fn(r)].append(r)
        result["by_" + g] = {k: agg(v) for k, v in buckets.items()}
    kinds = collections.defaultdict(lambda: {"n": 0, "procedural": 0, "hours": 0.0})
    for r in all_runs:
        for s in r["m"]["sendbacks"]:
            kinds[s["kind"]]["n"] += 1
            kinds[s["kind"]]["procedural"] += int(s["procedural"])
            kinds[s["kind"]]["hours"] += (s["latency_s"] or 0) / 3600
    result["sendback_kinds"] = kinds
    result["escalations"] = [e for r in all_runs for e in r["m"]["escalations"]]
    kinds_e = collections.Counter(e["kind"] for e in result["escalations"])
    result["escalation_kinds"] = dict(kinds_e)

    def jsonable(o):
        if isinstance(o, (dt.datetime,)):
            return o.isoformat()
        if isinstance(o, (set, collections.Counter)):
            return sorted(o) if isinstance(o, set) else dict(o)
        raise TypeError(type(o))

    if args.json_out:
        items = []
        for r in all_runs:
            m = {k: v for k, v in r["m"].items() if k != "_t"}
            t = r["m"].get("_t")
            items.append({"task": r["task"], "run": r["run"], "kind": r["kind"], "size": r["size"],
                          "profile": r["profile"], "harness": r["_harness"], "model": r["model"],
                          "outcome": r["outcome"], "metrics": m,
                          "transcript": None if not t else {
                              "help": t["help"], "invalid": t["invalid"], "done": t["done"], "rounds": t["rounds"],
                              "rejects": t["rejects"], "verbs": dict(t["verbs"])}})
        with open(args.json_out, "w") as fh:
            json.dump({**result, "items": items}, fh, indent=1, default=jsonable)
    md = render_md(result)
    if args.md_out:
        with open(args.md_out, "w") as fh:
            fh.write(md)
    else:
        sys.stdout.write(md)


if __name__ == "__main__":
    main()
