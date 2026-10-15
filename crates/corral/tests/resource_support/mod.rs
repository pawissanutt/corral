/// Live CLI fixtures test transport and candidate behavior, not host capacity.
/// Resource admission itself is tested with injectable probes in corral-ops.
pub fn configure(config: &mut corral_core::Config) {
    config.resources.max_load_per_core = f64::MAX;
    config.resources.min_mem_available = "1B".into();
    config.resources.min_disk_free = "1B".into();
}
