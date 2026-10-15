//! Resource warnings never prevent opening the board or attempt repairs.

use corral_ops::Ctx;

use super::Check;

pub(super) fn checks(ctx: &Ctx) -> Vec<Check> {
    let state = match corral_ops::resources::status(ctx) {
        Ok(state) => state,
        Err(error) => {
            return vec![Check {
                id: "resources",
                ok: false,
                hard: false,
                message: format!("Resource probe unavailable: {error:#}"),
                fix: None,
            }];
        }
    };
    let limits = &ctx.store.config.resources;
    let s = &state.snapshot;
    let thresholds = limits
        .min_mem_bytes()
        .and_then(|mem| limits.min_disk_bytes().map(|disk| (mem, disk)));
    let (mem, disk) = match thresholds {
        Ok(thresholds) => thresholds,
        Err(error) => {
            return vec![Check {
                id: "resources",
                ok: false,
                hard: false,
                message: format!("Resource thresholds unavailable: {error:#}"),
                fix: None,
            }];
        }
    };
    let mut checks = vec![
        Check {
            id: "resource_load",
            ok: s.load_per_core <= limits.max_load_per_core,
            hard: false,
            message: format!(
                "{:.2}/core ({:.2} / {} cores); maximum {:.2}/core",
                s.load_per_core, s.load_one, s.cores, limits.max_load_per_core
            ),
            fix: None,
        },
        Check {
            id: "resource_memory",
            ok: s.mem_available >= mem,
            hard: false,
            message: format!("{} bytes available; minimum {mem} bytes", s.mem_available),
            fix: None,
        },
    ];
    for d in &s.disks {
        checks.push(Check {
            id: "resource_disk",
            ok: d.free_bytes >= disk,
            hard: false,
            message: format!(
                "{}: {} bytes free; minimum {disk} bytes",
                d.path.display(),
                d.free_bytes
            ),
            fix: None,
        });
    }
    if let Some(reason) = state.reason {
        checks.push(Check {
            id: "resource_admission",
            ok: false,
            hard: false,
            message: format!("waiting: machine busy ({reason})"),
            fix: None,
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use corral_core::{DiskSpace, ResourceSnapshot, Store};
    use corral_ops::resources::FixedResourceProbe;

    #[test]
    fn resource_warnings_do_not_block_startup() {
        // Given a machine below its memory and disk thresholds.
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = corral_ops::Ctx::from_store(Store::init(dir.path()).unwrap());
        ctx.resource_probe = Arc::new(FixedResourceProbe(ResourceSnapshot {
            load_per_core: 2.0,
            load_one: 8.0,
            cores: 4,
            mem_available: 1,
            cpu_psi: None,
            mem_psi: None,
            disks: vec![DiskSpace {
                path: dir.path().into(),
                free_bytes: 1,
            }],
        }));
        // When doctor evaluates the injectable resource observation.
        let report = super::super::Report {
            checks: super::checks(&ctx),
            changes: Vec::new(),
        };
        // Then all resource failures remain soft and retain numerical evidence.
        assert!(report.startup_ready());
        assert!(!report.healthy());
        assert!(report.checks.iter().all(|check| !check.hard));
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.id == "resource_admission" && check.message.contains("2.00"))
        );
    }
}
