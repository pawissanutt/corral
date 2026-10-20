use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::Store;

use super::{DiskSpace, ResourceProbe, ResourceSnapshot, existing_parent};

#[derive(Debug, Clone, Copy)]
pub struct SystemResourceProbe;

impl ResourceProbe for SystemResourceProbe {
    fn sample(&self, store: &Store) -> Result<ResourceSnapshot> {
        let cores = u32::try_from(
            std::thread::available_parallelism()
                .context("reading CPU count")?
                .get(),
        )
        .context("CPU count exceeds u32")?;
        #[cfg(target_os = "linux")]
        let load_one =
            parse_load(&fs::read_to_string("/proc/loadavg").context("reading /proc/loadavg")?)?;
        #[cfg(target_os = "linux")]
        let mem_available =
            parse_mem(&fs::read_to_string("/proc/meminfo").context("reading /proc/meminfo")?)?;
        #[cfg(not(target_os = "linux"))]
        let (load_one, mem_available) = native_sample(cores)?;
        let mut paths = BTreeSet::from([store.dir().to_path_buf()]);
        for name in store.config.projects.keys() {
            let project = store.project_path(name)?;
            paths.insert(project.join(".worktrees"));
            paths.insert(project.join("target"));
        }
        for task in store.list_shared()?.iter() {
            for run in &task.runs {
                if run.worktree.exists() {
                    paths.insert(run.worktree.join("target"));
                    paths.insert(run.worktree.clone());
                }
            }
        }
        if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
            let target = PathBuf::from(target);
            paths.insert(if target.is_absolute() {
                target
            } else {
                std::env::current_dir()
                    .context("resolving CARGO_TARGET_DIR")?
                    .join(target)
            });
        }
        let disks = paths
            .into_iter()
            .map(|path| {
                let existing = existing_parent(&path)?;
                let free_bytes = fs2::available_space(&existing)
                    .with_context(|| format!("reading free space for {}", path.display()))?;
                Ok(DiskSpace { path, free_bytes })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ResourceSnapshot {
            load_per_core: load_one / f64::from(cores),
            load_one,
            cores,
            mem_available,
            cpu_psi: pressure("cpu")?,
            mem_psi: pressure("memory")?,
            disks,
        })
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn parse_load(text: &str) -> Result<f64> {
    let value: f64 = text
        .split_whitespace()
        .next()
        .context("missing one-minute load")?
        .parse()
        .context("invalid one-minute load")?;
    ensure!(value.is_finite() && value >= 0.0, "invalid one-minute load");
    Ok(value)
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn parse_mem(text: &str) -> Result<u64> {
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))
        .context("missing MemAvailable")?;
    let mut words = line.split_whitespace();
    let value: u64 = words
        .next()
        .context("missing MemAvailable value")?
        .parse()
        .context("invalid MemAvailable")?;
    ensure!(words.next() == Some("kB"), "invalid MemAvailable unit");
    value.checked_mul(1024).context("MemAvailable overflow")
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn parse_psi(text: &str) -> Result<f64> {
    let line = text
        .lines()
        .find(|l| l.starts_with("some "))
        .context("missing PSI some line")?;
    let value: f64 = line
        .split_whitespace()
        .find_map(|s| s.strip_prefix("avg10="))
        .context("missing PSI avg10")?
        .parse()
        .context("invalid PSI avg10")?;
    ensure!(
        value.is_finite() && (0.0..=100.0).contains(&value),
        "invalid PSI avg10"
    );
    Ok(value)
}

fn pressure(resource: &str) -> Result<Option<f64>> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = resource;
        Ok(None) // PSI is a Linux kernel facility, not an admission requirement.
    }
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/pressure/{resource}");
        match fs::read_to_string(&path) {
            Ok(text) => parse_psi(&text).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {path}")),
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn native_sample(cores: u32) -> Result<(f64, u64)> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let available = system.available_memory();
    ensure!(
        system.total_memory() > 0,
        "native memory inventory unavailable"
    );
    #[cfg(target_os = "macos")]
    let load = {
        let _ = cores;
        sysinfo::System::load_average().one
    };
    #[cfg(windows)]
    let load = {
        use std::process::{Command, Stdio};
        // Resolve English counter names through Perflib so non-English Windows works
        // too. PDH samples directly; a cold or unavailable WMI provider cannot make
        // admission hang or silently report an idle machine.
        let log = tempfile::tempfile().context("creating native resource probe output")?;
        let mut command = Command::new("powershell.exe");
        let probe = r#"
$ProgressPreference='SilentlyContinue'
$ErrorActionPreference='Stop'
$english=(Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Perflib\009').Counter
$localized=(Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Perflib\CurrentLanguage').Counter
$ids=@{}; $names=@{}
for ($i=0; $i+1 -lt $english.Length; $i+=2) { $ids[$english[$i+1]]=$english[$i] }
for ($i=0; $i+1 -lt $localized.Length; $i+=2) { $names[$localized[$i]]=$localized[$i+1] }
function LocalName([string]$name) {
  $id=$ids[$name]
  if ($null -eq $id -or $null -eq $names[$id]) { throw ('Performance counter unavailable: '+$name) }
  $names[$id]
}
$queueName=LocalName 'Processor Queue Length'
$cpuName=LocalName '% Processor Time'
$q='\'+(LocalName 'System')+'\'+$queueName
$c='\'+(LocalName 'Processor')+'(_Total)\'+$cpuName
$s=(Get-Counter -Counter @($q,$c) -MaxSamples 1 -ErrorAction Stop).CounterSamples
if ($s.Count -ne 2 -or @($s | Where-Object { $_.Status -gt 1 }).Count -ne 0) { throw 'Invalid native CPU counters' }
$queue=@($s | Where-Object { $_.Path.EndsWith('\'+$queueName,[StringComparison]::OrdinalIgnoreCase) })
$cpu=@($s | Where-Object { $_.Path.EndsWith('\'+$cpuName,[StringComparison]::OrdinalIgnoreCase) })
if ($queue.Count -ne 1 -or $cpu.Count -ne 1) { throw 'Incomplete native CPU counters' }
($queue[0].CookedValue + $cpu[0].CookedValue / 100.0 * CORRAL_CPU_CORES).ToString([Globalization.CultureInfo]::InvariantCulture)
"#.replace("CORRAL_CPU_CORES", &cores.to_string());
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &probe,
            ])
            .stdin(Stdio::null())
            .stdout(log.try_clone().context("cloning native probe output")?)
            .stderr(log.try_clone().context("cloning native probe errors")?);
        let (status, timed_out) = crate::hook_process::HookChild::spawn(command)?.wait(10)?;
        use std::io::{Read, Seek};
        let mut log = log;
        log.rewind().context("rewinding native resource output")?;
        let mut text = String::new();
        log.read_to_string(&mut text)
            .context("reading native resource output")?;
        ensure!(
            status.success() && !timed_out,
            "native resource counters unavailable: {status}; {text}"
        );
        text.trim()
            .parse::<f64>()
            .context("parsing native CPU demand")?
    };
    ensure!(load.is_finite() && load >= 0.0, "invalid native CPU demand");
    Ok((load, available))
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_resource_inventory_observes_memory_and_writable_volume() {
        use super::ResourceProbe;
        // Given a private store without an engine or external services.
        let root = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(root.path()).unwrap();
        // When the real platform resource probe samples the machine.
        let snapshot = super::SystemResourceProbe.sample(&store).unwrap();
        // Then resource admission uses native memory, CPU and volume observations.
        assert!(snapshot.cores > 0);
        assert!(snapshot.load_one.is_finite() && snapshot.load_one >= 0.0);
        assert!(snapshot.mem_available > 0);
        assert!(!snapshot.disks.is_empty());
    }
}
