use serde::{Deserialize, Serialize};

/// `[search]`: optional content-search acceleration; streaming search remains the default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchCfg {
    pub catalog: bool,
}

#[cfg(test)]
mod tests {
    use super::SearchCfg;
    use crate::{Config, ConfigDoc};

    #[test]
    fn catalog_defaults_remain_visible_and_round_trip_as_booleans() {
        let mut doc = ConfigDoc::parse("").unwrap();
        let config = doc.config().unwrap();
        assert_eq!(config.search, SearchCfg::default());
        assert_eq!(
            config.effective_value().unwrap()["search"]["catalog"].as_bool(),
            Some(false)
        );
        let serialized = toml::to_string(&config).unwrap();
        let restored: Config = toml::from_str(&serialized).unwrap();
        assert!(!restored.search.catalog);

        doc.set_str("search.catalog", "true").unwrap();
        let enabled = doc.config().unwrap();
        assert!(enabled.search.catalog);
        let restored: Config = toml::from_str(&toml::to_string(&enabled).unwrap()).unwrap();
        assert!(restored.search.catalog);

        doc.set_str("search.catalog", "\"true\"").unwrap();
        assert!(doc.config().is_err());
        assert!(toml::from_str::<SearchCfg>("catalogue = true").is_err());
    }
}
