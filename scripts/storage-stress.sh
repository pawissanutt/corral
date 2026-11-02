#!/usr/bin/env bash
# The stress workload never runs on the host. Ordinary compilation happens first;
# all crash/concurrency tests then execute in one resource-capped disposable container.
set -euo pipefail
exec </dev/null
root=$(git rev-parse --show-toplevel)
cd "$root"
out=${1:-.evidence/storage-stress}
mkdir -p "$out"
out=$(realpath "$out")
[[ $out == "$root/"* ]] || { echo 'output must be inside the mounted worktree' >&2; exit 2; }
cargo test -p corral-core --test sqlite_stress --test backend_conformance_sqlite --test blob_gc \
    --no-run --message-format=json >"$out/build.jsonl" 2>"$out/build.stderr"
python3 -I - "$out/build.jsonl" "$out/tests.sh" <<'PY'
import json, shlex, sys
bins = {}
for line in open(sys.argv[1]):
    row = json.loads(line)
    if row.get('reason') == 'compiler-artifact' and row.get('executable'):
        bins[row['target']['name']] = row['executable']
commands = [('sqlite_stress', ['--ignored']),
            ('backend_conformance_sqlite', ['claim_race_has_one_winner']),
            ('backend_conformance_sqlite', ['backups_taken_while_a_writer_commits_are_consistent']),
            ('backend_conformance_sqlite', ['eight_processes_open_a_fresh_database_ten_times']),
            ('backend_conformance_sqlite', ['a_view_in_another_process_blocks_no_writer_and_its_drop_lets_the_wal_truncate']),
            ('blob_gc', [])]
with open(sys.argv[2], 'w') as f:
    f.write('set -euo pipefail\n')
    for name, filters in commands:
        f.write(shlex.join([bins[name], *filters, '--nocapture', '--test-threads=1'])+'\n')
    f.write('echo "STRESS PASSED: 30 kills, claim race, backup, open race, held view, blob GC"\n')
PY
# pipefail preserves Podman/test refusal. No host fallback, retries or synthetic pass.
scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 -- \
    bash "$out/tests.sh" 2>&1 | tee "$out/results.log"
