#!/usr/bin/env python3
"""Small standalone smoke test; no Corral configuration required."""
import json
from pathlib import Path
import subprocess
import tempfile

scripts = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory() as directory:
    repo = Path(directory)
    def git(*args):
        return subprocess.run(["git", "-C", directory, *args], check=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True).stdout.strip()
    git("init", "-q")
    (repo / "code.txt").write_text("alpha login\n", encoding="utf-8")
    (repo / "tags").write_text('login\t./code.txt\t1;"\tkind:function\n', encoding="utf-8")
    git("add", "code.txt", "tags")
    git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "fixture")
    rev = git("rev-parse", "HEAD")
    (repo / "tags").write_text("uncommitted change\n", encoding="utf-8")
    def call(script, request):
        result = subprocess.run(["python3", str(scripts / script)], input=json.dumps(request),
                                text=True, capture_output=True, check=True)
        return result.stdout
    output = call("context.py", {"v": 1, "repo": directory, "task": {"title": "login"}})
    assert "code.txt:1:alpha login" in output, output
    rows = [json.loads(row) for row in call("facts.py", {"v": 1, "repo": directory,
            "query": "snapshot", "rev": rev, "want": ["definitions"]}).splitlines()]
    assert rows[0] == {"type": "definition", "path": "code.txt", "name": "login",
                       "line": 1, "end_line": 1, "kind": "function"}, rows
    assert rows[-1]["provided"] == ["definitions"], rows
    delta = call("facts.py", {"v": 1, "query": "delta", "want": ["changes"]})
    assert json.loads(delta)["provided"] == []
print("extension examples: OK")
