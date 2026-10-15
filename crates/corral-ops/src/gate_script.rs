use std::path::Path;

use anyhow::Result;
use corral_core::CandidateIdentity;

use crate::gate::GatePaths;
use crate::prompt::sh_escape;

#[cfg(test)]
#[path = "gate_script_tests.rs"]
mod tests;

pub(crate) fn q(value: &str) -> String {
    format!("'{}'", sh_escape(value))
}

pub(crate) fn render(
    identity: &CandidateIdentity,
    worktree: &Path,
    commands: &[String],
    paths: &GatePaths,
) -> Result<String> {
    let cmds: String = commands.iter().map(|c| format!("  {}\n", q(c))).collect();
    // Apply captured values only in each command shell, never in runner bookkeeping.
    // Even names matching runner scalars or arrays must retain their configured values.
    let environment: String = identity
        .environment
        .iter()
        .map(|(name, value)| match value {
            Some(value) => format!("export {name}={}\n", q(value)),
            None => format!("unset {name}\n"),
        })
        .collect();
    let json_cmds: String = commands
        .iter()
        .map(|c| serde_json::to_string(c).map(|s| format!("  {}\n", q(&s))))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .concat();
    Ok(format!(
        r#"#!/usr/bin/env bash
set -u
umask 077
exec </dev/null
command_environment={command_environment}
# Do not inherit an exported variable's export attribute from an enclosing gate.
unset owner_token CORRAL_GATE_OWNER_TOKEN
owner_token=$(< {owner})
if [ -z "$owner_token" ]; then exit 1; fi
log={log}
result={result}
failed={failed}
identity={identity}
export CORRAL_INTEGRATION_BASE={base}
export CORRAL_CANDIDATE_HEAD={head}
export CORRAL_GATE_ATTEMPT={attempt}
receipts=''
runner={runner}
process=$(awk '{{print $22}}' /proc/$$/stat)
boot=$(< /proc/sys/kernel/random/boot_id)
runner_started=$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)
runner_progress() {{
  printf '{{"pid":%d,"process":"%s","boot":"%s","state":"%s","started":"%s","ended":%s}}\n' "$$" "$process" "$boot" "$1" "$runner_started" "$2" > "$runner.tmp" && mv -f "$runner.tmp" "$runner"
}}
command_progress() {{
  progress={command_prefix}
  progress="$progress$i"
  printf '{{"command":%s,"state":"%s","started":"%s","ended":%s,"duration_ms":%s,"exit_code":%s,"log_start":%d,"log_end":%d}}\n' "${{json_cmds[$i]}}" "$1" "$command_started" "$2" "$3" "$4" "$start" "$5" > "$progress.tmp" && mv -f "$progress.tmp" "$progress.json"
}}
runner_progress running null
finish() {{
  printf '\ncorral gate: %s\n' "$1" | tee -a "$log"
  printf '{{"schema_version":2,"owner_token":"%s","identity":%s,"result":"%s","exit_code":%d,"commands":[%s]}}\n' "$owner_token" "$identity" "$1" "$2" "$receipts" > "$result.tmp" && mv -f "$result.tmp" "$result"
  state=failed
  if [ "$1" = pass ]; then state=passed; fi
  runner_progress "$state" "\"$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)\""
}}
cmds=(
{cmds})
json_cmds=(
{json_cmds})
: > "$log"
if ! cd {wt}; then
  printf '%s' 'worktree unavailable' > "$failed"
  finish fail 1
  exit 1
fi
for i in "${{!cmds[@]}}"; do
  c="${{cmds[$i]}}"
  start=$(wc -c < "$log")
  command_started=$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)
  command_micros="${{EPOCHREALTIME/./}}"
  command_progress running null null null "$start"
  printf '\n$ %s\n' "$c" | tee -a "$log"
  bash -o pipefail -c "$command_environment$c" </dev/null 2>&1 | tee -a "$log"
  codes=("${{PIPESTATUS[@]}}")
  code="${{codes[0]}}"
  if [ "${{codes[1]}}" -ne 0 ]; then code="${{codes[1]}}"; fi
  end=$(wc -c < "$log")
  end_micros="${{EPOCHREALTIME/./}}"
  duration_ms=$(( (end_micros - command_micros) / 1000 ))
  state=passed
  if [ "$code" -ne 0 ]; then state=failed; fi
  command_progress "$state" "\"$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)\"" "$duration_ms" "$code" "$end"
  receipt=$(printf '{{"command":%s,"exit_code":%d,"log_start":%d,"log_end":%d}}' "${{json_cmds[$i]}}" "$code" "$start" "$end")
  if [ -n "$receipts" ]; then receipts+=','; fi
  receipts+="$receipt"
  if [ "$code" -ne 0 ]; then
    printf '%s' "$c" > "$failed"
    finish fail "$code"
    exit "$code"
  fi
done
finish pass 0
"#,
        log = q(&paths.log.display().to_string()),
        command_prefix = q(&paths.log.with_extension("command-").display().to_string()),
        runner = q(&paths
            .log
            .with_extension("runner.json")
            .display()
            .to_string()),
        owner = q(&paths.result.with_extension("owner").display().to_string()),
        result = q(&paths.result.display().to_string()),
        failed = q(&paths.failed.display().to_string()),
        identity = q(&serde_json::to_string(identity)?),
        base = q(&identity.integration_base),
        head = q(&identity.head),
        attempt = q(&identity.attempt),
        wt = q(&worktree.display().to_string()),
        command_environment = q(&environment),
    ))
}
