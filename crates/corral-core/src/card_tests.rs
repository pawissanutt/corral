use super::*;

#[test]
fn defaults_show_cost_and_stay_out_of_the_serialized_config() {
    let cfg = Config::default_for(&std::env::temp_dir());
    assert_eq!(
        cfg.board.card.fields_for(Status::Running),
        [CardField::Cost]
    );
    assert!(!cfg.to_toml().contains("[board"), "{}", cfg.to_toml());
}

#[test]
fn card_section_parses_and_per_column_override_wins() {
    let cfg: Config = toml::from_str(
        "[board.card]\nfields = [\"cost\", \"age\"]\ndensity = \"rich\"\ncost_warn = 10.0\ncolumn_cost = true\n\n[board.card.columns]\nrunning = [\"stage\"]\n",
    )
    .unwrap();
    assert_eq!(cfg.board.card.density, Density::Rich);
    assert_eq!(cfg.board.card.cost_warn, Some(10.0));
    assert_eq!(
        cfg.board.card.fields_for(Status::Review),
        [CardField::Cost, CardField::Age]
    );
    assert_eq!(
        cfg.board.card.fields_for(Status::Running),
        [CardField::Stage]
    );
    assert!(cfg.to_toml().contains("[board.card]"));
}

#[test]
fn validation_names_unknown_fields_columns_and_bad_warning() {
    let mut cfg = Config::default_for(&std::env::temp_dir());
    cfg.board.card.fields = vec!["cost".into(), "bogus".into()];
    cfg.board
        .card
        .columns
        .insert("nowhere".into(), vec!["age".into()]);
    cfg.board.card.cost_warn = Some(0.0);
    let err = cfg.validate(&std::env::temp_dir()).unwrap_err().to_string();
    assert!(
        err.contains("board.card.fields: unknown field 'bogus'"),
        "{err}"
    );
    assert!(
        err.contains("board.card.columns.nowhere: unknown column"),
        "{err}"
    );
    assert!(err.contains("board.card.cost_warn must be > 0"), "{err}");
}

#[test]
fn fit_drops_the_last_fields_first_and_keeps_one() {
    let parts = vec![
        (1, "$1.42".to_string()),
        (2, "sonnet".to_string()),
        (3, "12m".to_string()),
    ];
    assert_eq!(CardField::fit(parts.clone(), 40).len(), 3);
    assert_eq!(CardField::fit(parts.clone(), 14).len(), 2);
    assert_eq!(CardField::fit(parts, 1), [(1, "$1.42".to_string())]);
}

#[test]
fn counts_and_ages_are_short() {
    assert_eq!(short_count(950), "950");
    assert_eq!(short_count(48_200), "48.2k");
    assert_eq!(short_count(3_400_000), "3.4M");
    assert_eq!(age_text(59), "59s");
    assert_eq!(age_text(3_900), "1h05m");
    assert_eq!(age_text(200_000), "2d");
}

#[test]
fn every_field_round_trips_its_key() {
    for f in CardField::ALL {
        assert_eq!(CardField::parse(f.key()), Some(f));
    }
}
