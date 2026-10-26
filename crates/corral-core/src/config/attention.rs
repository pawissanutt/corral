use super::PmCfg;

pub(super) const fn seconds() -> u64 {
    900
}
pub(super) const fn owner_seconds() -> u64 {
    1800
}

pub(super) fn problems(pm: &PmCfg) -> Vec<String> {
    let mut out = Vec::new();
    if !(60..=86400).contains(&pm.attention_secs) {
        out.push("pm.attention_secs must be 60..86400".into());
    }
    if pm.attention_owner_secs <= pm.attention_secs || pm.attention_owner_secs > 604800 {
        out.push(
            "pm.attention_owner_secs must exceed pm.attention_secs and be at most 604800".into(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_threshold_bounds_name_the_rejected_configuration_key() {
        // Given defaults and every boundary of the two independent configuration values.
        let mut pm = PmCfg::default();
        assert!(problems(&pm).is_empty());
        for (seconds, valid) in [(59, false), (60, true), (86400, true), (86401, false)] {
            pm.attention_secs = seconds;
            pm.attention_owner_secs = 604800;
            // When validated, then only values inside the adopted interval succeed.
            assert_eq!(problems(&pm).is_empty(), valid);
            if !valid {
                assert!(
                    problems(&pm)
                        .iter()
                        .any(|problem| problem.starts_with("pm.attention_secs"))
                );
            }
        }
        pm.attention_secs = 900;
        for (seconds, valid) in [(900, false), (901, true), (604800, true), (604801, false)] {
            pm.attention_owner_secs = seconds;
            assert_eq!(problems(&pm).is_empty(), valid);
            if !valid {
                assert!(
                    problems(&pm)
                        .iter()
                        .any(|problem| problem.starts_with("pm.attention_owner_secs"))
                );
            }
        }
    }
}
