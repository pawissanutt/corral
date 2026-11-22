#!/usr/bin/env python3
"""Read numeric-line ctags from a tracked file at the requested revision."""
import json
import posixpath
import subprocess
import sys

request = json.load(sys.stdin)
provided = []
skipped = 0
if request.get("query") == "snapshot" and "definitions" in request.get("want", []):
    if not request.get("sample"):
        tags = sys.argv[1] if len(sys.argv) > 1 else "tags"
        result = subprocess.run(
            ["git", "-C", request["repo"], "show", f"{request['rev']}:{tags}"],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        if result.returncode:
            sys.stderr.write(result.stderr)
            sys.exit(result.returncode)
        for row in result.stdout.splitlines():
            fields = row.split("\t")
            if row.startswith("!_TAG_") or len(fields) < 3:
                continue
            number = fields[2].removesuffix(';"')
            if not number.isascii() or not number.isdecimal() or len(number) > 20 or not 0 < int(number) < 2**64:
                skipped += 1
                continue
            path = posixpath.normpath(fields[1])
            if path in (".", "..") or path.startswith("../") or path.startswith("/"):
                skipped += 1
                continue
            kind = next((f[5:] for f in fields[3:] if f.startswith("kind:")), "")
            if not kind:
                kind = next((f for f in fields[3:] if ":" not in f), "unknown")
            print(json.dumps({"type": "definition", "path": path, "name": fields[0],
                              "line": int(number), "end_line": int(number), "kind": kind}))
    provided.append("definitions")
print(json.dumps({"type": "done", "provided": provided, "skipped": {"unsupported_tag": skipped} if skipped else {}, "omitted": {}}))
