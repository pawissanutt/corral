use std::path::Path;

use anyhow::Result;
use corral_core::CandidateIdentity;

use crate::gate::GatePaths;
use crate::gate_tier::GateContext;
use crate::prompt::sh_escape;
use crate::prompt::shell_path;

#[cfg(test)]
#[path = "gate_script_tests.rs"]
mod tests;

pub(crate) fn q(value: &str) -> String {
    format!("'{}'", sh_escape(value))
}

pub(crate) fn qp(path: &Path) -> String {
    q(&shell_path(path))
}

pub(crate) fn render(
    identity: &CandidateIdentity,
    worktree: &Path,
    commands: &[String],
    paths: &GatePaths,
    context: Option<&GateContext>,
) -> Result<String> {
    // Runner exports like the integration base: never part of the captured environment.
    let tier_exports = context.map_or_else(String::new, |context| {
        format!(
            "export CORRAL_GATE_TIER={}\nexport CORRAL_GATE_CONTEXT={}\n",
            q(&context.tier),
            qp(&context.path)
        )
    });
    let cmds: String = commands.iter().map(|c| format!("  {}\n", q(c))).collect();
    // Apply captured values only in each command shell, never in runner bookkeeping.
    // Even names matching runner scalars or arrays must retain their configured values.
    let environment: String = identity
        .environment
        .iter()
        .map(|(name, value)| match value {
            Some(value) => {
                #[cfg(windows)]
                if name == "PATH" {
                    return format!("export PATH=$(cygpath -up -- {})\n", q(value));
                }
                format!("export {name}={}\n", q(value))
            }
            None => format!("unset {name}\n"),
        })
        .collect();
    let json_cmds: String = commands
        .iter()
        .map(|c| serde_json::to_string(c).map(|s| format!("  {}\n", q(&s))))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .concat();
    #[cfg(target_os = "linux")]
    let process_clock = r#"native_pid=$$
process=$(awk '{print $22}' /proc/$$/stat)
boot=$(< /proc/sys/kernel/random/boot_id)
monotonic_millis() {
  awk '{printf "%.0f\n", $1 * 1000}' /proc/uptime
}"#
    .to_string();
    #[cfg(not(target_os = "linux"))]
    let process_clock = {
        #[cfg(windows)]
        let native_pid = "native_pid=$(< /proc/$$/winpid)";
        #[cfg(not(windows))]
        let native_pid = "native_pid=$$";
        format!(
            r#"probe={probe}
{native_pid}
process=$("$probe" runtime-probe --pid "$native_pid") || exit 1
boot=native
monotonic_millis() {{
  "$probe" runtime-probe
}}"#,
            probe = qp(&crate::prompt::corral_bin()?)
        )
    };
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
{tier_exports}receipts=''
runner={runner}
{process_clock}
runner_started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
runner_progress() {{
  printf '{{"pid":%d,"process":"%s","boot":"%s","state":"%s","started":"%s","ended":%s}}\n' "$native_pid" "$process" "$boot" "$1" "$runner_started" "$2" > "$runner.tmp" && mv -f "$runner.tmp" "$runner"
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
  runner_progress "$state" "\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\""
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
  command_started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  command_millis=$(monotonic_millis)
  command_progress running null null null "$start"
  printf '\n$ %s\n' "$c" | tee -a "$log"
  bash -o pipefail -c "$command_environment$c" </dev/null 2>&1 | tee -a "$log"
  codes=("${{PIPESTATUS[@]}}")
  code="${{codes[0]}}"
  if [ "${{codes[1]}}" -ne 0 ]; then code="${{codes[1]}}"; fi
  end=$(wc -c < "$log")
  end_millis=$(monotonic_millis)
  duration_ms=$(( end_millis - command_millis ))
  if [ "$duration_ms" -lt 0 ]; then duration_ms=0; fi
  state=passed
  if [ "$code" -ne 0 ]; then state=failed; fi
  command_ended=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  command_progress "$state" "\"$command_ended\"" "$duration_ms" "$code" "$end"
  receipt=$(printf '{{"command":%s,"exit_code":%d,"log_start":%d,"log_end":%d,"started":"%s","ended":"%s"}}' "${{json_cmds[$i]}}" "$code" "$start" "$end" "$command_started" "$command_ended")
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
        log = qp(&paths.log),
        command_prefix = qp(&paths.log.with_extension("command-")),
        runner = qp(&paths.log.with_extension("runner.json")),
        owner = qp(&paths.result.with_extension("owner")),
        result = qp(&paths.result),
        failed = qp(&paths.failed),
        identity = q(&serde_json::to_string(identity)?),
        base = q(&identity.integration_base),
        head = q(&identity.head),
        attempt = q(&identity.attempt),
        wt = qp(worktree),
        command_environment = q(&environment),
    ))
}
