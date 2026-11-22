#!/usr/bin/env python3
"""Find tracked worktree lines containing words from the current task title."""
import json
import re
import subprocess
import sys

request = json.load(sys.stdin)
title = (request.get("task") or {}).get("title", "")
words = list(dict.fromkeys(re.findall(r"[\w-]{3,}", title)))[:12]
if not words:
    print("No task title words to search for.")
    sys.exit(0)
command = ["git", "-C", request.get("repo") or ".", "grep", "-n", "-I", "-i", "-F"]
for word in words:
    command.extend(["-e", word])
# Stream bounded chunks so even unusually long matching lines cannot exhaust memory.
process = subprocess.Popen(command + ["--"], stdout=subprocess.PIPE)
assert process.stdout is not None
count = 0
truncated = False
while count < 100:
    line = process.stdout.readline(3001)
    if not line:
        break
    if len(line) > 3000 or not line.endswith(b"\n"):
        print(line[:3000].decode("utf-8", errors="replace"))
        truncated = True
        break
    print(line.decode("utf-8", errors="replace"), end="")
    count += 1
if count == 100:
    truncated = True
if truncated:
    process.terminate()
    print("(results truncated)")
process.stdout.close()
status = process.wait()
if not truncated and status not in (0, 1):
    sys.exit(status)
if not truncated and count == 0:
    print("No matches.")
