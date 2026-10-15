use super::*;

#[test]
fn resources_config_defaults_and_partial_overrides() {
    let cfg: crate::Config = toml::from_str("[resources]\nmin_disk_free = '2GB'").unwrap();
    assert_eq!(cfg.resources.min_disk_bytes().unwrap(), 2_000_000_000);
    assert_eq!(cfg.resources.min_mem_bytes().unwrap(), 8_000_000_000);
    assert!(cfg.resources.problems().is_empty());
}

#[test]
fn resources_config_rejects_invalid_thresholds() {
    for load in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        let cfg = ResourcesCfg {
            max_load_per_core: load,
            ..ResourcesCfg::default()
        };
        assert!(!cfg.problems().is_empty());
    }
    for value in ["0", "0GB", "-1GB", "nonsense"] {
        let cfg = ResourcesCfg {
            min_mem_available: value.into(),
            min_disk_free: value.into(),
            ..ResourcesCfg::default()
        };
        assert_eq!(cfg.problems().len(), 2);
    }
}
