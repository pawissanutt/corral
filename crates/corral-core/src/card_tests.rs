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

#[test]
fn blocked_configuration_accepts_new_and_legacy_keys() {
    // Given a saved legacy override and a newer Blocked override.
    let mut cfg = CardCfg::default();
    cfg.columns.insert("needs_me".into(), vec!["age".into()]);
    cfg.columns.insert("blocked".into(), vec!["cost".into()]);
    // When selecting the rendered column's fields, then the new key wins without losing old parsing.
    assert_eq!(cfg.fields_for(Status::NeedsMe), [CardField::Cost]);
    assert_eq!(CardField::parse("needs_me"), Some(CardField::NeedsMe));
    assert_eq!(CardField::parse("blocked"), Some(CardField::NeedsMe));
    assert!(cfg.problems().is_empty());
    assert_eq!(Status::parse("blocked"), Some(Status::NeedsMe));
    assert_eq!(Status::parse("needs_me"), Some(Status::NeedsMe));
    assert_eq!(serde_json::to_value(Status::NeedsMe).unwrap(), "needs_me");
}

#[test]
fn ideas_use_the_backlog_override_because_they_render_in_that_column() {
    let mut cfg = CardCfg::default();
    cfg.columns.insert("backlog".into(), vec!["tokens".into()]);
    assert_eq!(cfg.fields_for(Status::Backlog), [CardField::Tokens]);
    assert_eq!(cfg.fields_for(Status::Idea), [CardField::Tokens]);
    assert_eq!(cfg.fields_for(Status::Ready), [CardField::Cost]);
}

#[test]
fn fit_measures_terminal_cells_not_characters() {
    let wide = vec![(1, "界".to_string()), (2, "$1.50".to_string())];
    assert_eq!(
        CardField::fit(wide, 9),
        [(1, "界".to_string())],
        "2 + 3 + 5 cells do not fit in 9"
    );
    let combining = vec![(1, "e\u{301}e\u{301}".to_string()), (2, "$1.5".to_string())];
    assert_eq!(
        CardField::fit(combining, 9).len(),
        2,
        "2 + 3 + 4 cells fit in 9 although there are 11 chars"
    );
}
