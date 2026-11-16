"""Entry point for candidate-bound isolated recovery QA; failures are never skips."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

from fixtures import BIN, Fixture, digest, save
from surface import surface


def herdr(*args):
    result = subprocess.run(["herdr", "--session", "corral-test", *args],
                            stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=20)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def cleanup(evidence):
    root = Path(os.environ["CORRAL_TEST_HERDR_ROOT"])
    assert root.parent == Path("/tmp") and root.name.startswith("corral-herdr-")
    for fixture in (root / "recovery-fixtures").glob("*"):
        result = subprocess.run([BIN, "--root", str(fixture), "daemon", "stop", "--drain"],
                                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
        save(evidence / f"cleanup-{fixture.name}.json", {"exit": result.returncode,
             "stdout": result.stdout, "stderr": result.stderr})
        assert result.returncode in (0, 2), f"daemon stop {fixture}: {result}"
    before = herdr("workspace", "list")
    save(evidence / "cleanup-before.json", before)
    for workspace in before["result"]["workspaces"]:
        herdr("workspace", "close", workspace["workspace_id"])
    save(evidence / "cleanup-after.json", herdr("workspace", "list"))


def close_case_workspaces(f):
    task = json.loads((f.ev / "final.json").read_text())
    refs = [run.get("herdr") for run in task.get("plan_runs", [])]
    refs += [review.get("herdr") for plan in task["plans"] for review in plan.get("reviews", [])]
    owned = {ref["workspace_id"] for ref in refs if ref}
    before = {w["workspace_id"] for w in herdr("workspace", "list")["result"]["workspaces"]}
    for workspace in sorted(owned & before):
        herdr("workspace", "close", workspace)
    after = {w["workspace_id"] for w in herdr("workspace", "list")["result"]["workspaces"]}
    assert after == before - owned, "case cleanup changed an unrelated fixture workspace"
    save(f.ev / "workspace-cleanup.json", {"owned": sorted(owned), "before": sorted(before), "after": sorted(after)})


def family_refusals(f):
    for reviewer in [[], ["--reviewer", "gpt-astra"]]:
        error = f.c("plan", "review", f.id, *reviewer, ok=False)
        assert "cross-family review is required" in error and "openai" in error, error
    assert f.task()["plans"][-1].get("reviews", []) == []
    preview = f.j(*f.reviewer_args(), "--json")
    assert preview["request"]["exceptions"] == ["reviewer-family-policy", "configured-reviewer-binding"]
    assert preview["policy"]["resolution_failure"] is None
    assert preview["policy"]["digest"]
    assert preview["policy"]["inputs"]["effective_policy"]["require_cross_family"] is True
    assert preview["policy"]["sources"]["reviewer-family-policy"] == "[projects.app.review]"
    assert preview["policy"]["expected_reviewer"]["profile"] == "gpt-sol"
    save(f.ev / "family-and-binding-preview.json", preview)


def cli(f, name):
    if name == "N-cli":
        review = f.j("plan", "review", f.id, "--json")
        assert review["reviewer"] == "gpt-sol"
        f.callback(review["id"])
        f.c("plan", "verdict", f.id, "approve", "--owner", "--plan", "P1", "--notes-file", "notes.md")
        f.finish(False)
    elif name == "F-cli":
        family_refusals(f)
        f.apply_cli(f.reviewer_args(), "review")
        f.callback("PR1")
        f.apply_cli(f.accept_args(), "accept")
        f.finish(True)
    elif name == "M-ordinary":
        review = f.j("plan", "review", f.id, "--reviewer", "gpt-astra", "--json")
        f.callback(review["id"], "escalate")
        assert "unauthorized reviewer" in f.task()["plans"][-1]["reviews"][-1]["notes"]
        f.c("route", f.id, "--set-reviewer", "gpt-astra")
        second = f.j("plan", "review", f.id, "--json")
        f.callback(second["id"])
        f.c("plan", "verdict", f.id, "approve", "--owner", "--plan", "P1", "--notes-file", "notes.md")
        f.finish(False)
    elif name == "M-recovery":
        preview = f.j(*f.reviewer_args(), "--json")
        assert preview["request"]["exceptions"] == ["configured-reviewer-binding"]
        family = next(check for check in preview["checks"] if check["check"] == "reviewer-family-policy")
        assert family["observed"] == {"kind": "predicate", "satisfied": True}
        assert family["disposition"] == "satisfied"
        f.apply_cli(f.reviewer_args(), "review")
        f.callback("PR1")
        f.apply_cli(f.accept_args(), "accept")
        f.finish(True)


def run(evidence, historical):
    private = Path(os.environ["CORRAL_TEST_HERDR_ROOT"])
    assert Path(os.environ["HOME"]).is_relative_to(private)
    assert Path(os.environ["XDG_CONFIG_HOME"]).is_relative_to(private)
    save(evidence / "identity.json", {"candidate": BIN, "binary_sha256": digest(Path(BIN).read_bytes()),
         "private_root": str(private), "home": os.environ["HOME"],
         "xdg_config": os.environ["XDG_CONFIG_HOME"], "fixture_reviewers": "inert; no model calls",
         "terminal_backend": "real controlling PTY; keyboard bytes and SGR mouse", "sizes": [[80, 24], [120, 40]],
         "driver_sha256": {p.name: digest(p.read_bytes()) for p in Path(__file__).parent.glob("*.py")}})
    save(evidence / "initial-workspaces.json", herdr("workspace", "list"))
    config = Path(os.environ["XDG_CONFIG_HOME"])
    hashes = lambda: {str(p.relative_to(config)): digest(p.read_bytes()) for p in config.rglob("*.toml")}
    before = hashes()
    assert historical.is_dir(), "supply retained H evidence; never reproduce H with today's default"
    shutil.copytree(historical, evidence / "H")
    base = private / "recovery-fixtures"
    base.mkdir()
    results = []
    cases = ["N-cli", "F-cli", "M-ordinary", "M-recovery", "N-keyboard", "N-mouse", "F-keyboard", "F-mouse", "M-keyboard", "M-mouse"]
    selected = os.environ.get("RECOVERY_QA_CASES", "")
    requested = selected.split(",") if selected else cases
    assert set(requested) <= set(cases), requested
    save(evidence / "coverage.json", {"required": cases, "selected": requested, "all_cases_selected": requested == cases,
         "acceptance_complete": False, "not_verified": ["N acceptance_mode=policy", "N policy_satisfied=true"],
         "followup": "T-971"})
    try:
        for name in requested:
            print(f"START {name}", flush=True)
            started = time.monotonic()
            f = Fixture(base, evidence, name, family=name.startswith("F-"))
            if name.endswith(("keyboard", "mouse")):
                if name.startswith("F-"):
                    family_refusals(f)
                surface(f, mouse=name.endswith("mouse"), recovered=not name.startswith("N-"))
            else:
                cli(f, name)
            close_case_workspaces(f)
            results.append({"case": name, "status": "pass", "seconds": round(time.monotonic() - started, 3),
                            "not_verified": ["acceptance_mode=policy", "policy_satisfied=true"] if name.startswith("N-") else [],
                            "followup": "T-971" if name.startswith("N-") else None})
            save(evidence / "results.json", results)
            print(f"PASS {name}", flush=True)
        assert hashes() == before, "private global config changed across recovery"
        save(evidence / "global-config-hashes.json", {"before": before, "after": hashes()})
        save(evidence / "limitations.json", {"H": "retained only, no historical binary reproduction",
             "ordinary_plan_readback": "NOT VERIFIED (T-971): no acceptance_mode/policy_satisfied fields in PlanApproval; ordinary digest/approval/no-recovery checked, fields not invented",
             "quality": "inert reviewers prove callback identity, not review quality",
             "coverage": "operation-level freshness, resources, authority, crash and schema tests run separately"})
    except Exception as error:
        results.append({"case": name, "status": "fail", "error": str(error)})
        save(evidence / "results.json", results)
        raise


if __name__ == "__main__":
    evidence = Path(sys.argv[2]).resolve()
    if sys.argv[1] == "cleanup":
        cleanup(evidence)
    else:
        run(evidence, Path(sys.argv[3]).resolve())
