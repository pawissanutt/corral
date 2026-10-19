//! Stage profiling and a bounded Settings hot-path regression measurement.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use corral_core::Store;
use corral_ops::Ctx;
use ratatui::Terminal;

use super::App;
use crate::Mode;
use crate::View;

#[test]
fn settings_draw_and_unchanged_refresh_stay_responsive() {
    // Given a workspace with the per-task automation scope that grows over time.
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::init(root.path()).unwrap();
    for n in 0..256 {
        store.config.automation.scope.tasks.insert(
            format!("T-{n:03}"),
            corral_core::ScopedTask {
                project: "app".into(),
                repository: root.path().into(),
                repository_identity: "a".repeat(64),
                contract_revision: "b".repeat(64),
            },
        );
    }
    std::fs::write(store.config_path(), store.config.to_toml()).unwrap();
    let ctx = Ctx::from_store(Store::open(root.path()).unwrap());
    let mut view = crate::settings::SettingsView::new(&ctx);
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 45)).unwrap();
    // When the open Settings screen redraws and receives unchanged syncs.
    let start = Instant::now();
    for _ in 0..10 {
        terminal
            .draw(|frame| view.draw(frame, frame.area(), &ctx, &mut crate::HitMap::default()))
            .unwrap();
    }
    let draw = start.elapsed() / 10;
    let start = Instant::now();
    for _ in 0..100 {
        view.refresh(&ctx);
    }
    let refresh = start.elapsed() / 100;
    println!("settings_draw_ms={:.3}", draw.as_secs_f64() * 1000.0);
    println!("settings_refresh_ms={:.3}", refresh.as_secs_f64() * 1000.0);
    // Then both stay well below the 200 ms input poll interval, even in debug builds.
    assert!(draw < Duration::from_millis(50), "Settings draw: {draw:?}");
    assert!(
        refresh < Duration::from_millis(1),
        "unchanged Settings refresh: {refresh:?}"
    );
}

#[test]
#[ignore = "requires CORRAL_LATENCY_FIXTURE from scripts/qa/board-latency.mjs"]
fn profile_board_stages() {
    let root = PathBuf::from(std::env::var_os("CORRAL_LATENCY_FIXTURE").unwrap());
    let ctx = Ctx::from_store(Store::open(&root).unwrap());
    let measure = |label: &str, action: &mut dyn FnMut()| {
        for sample in 0..5 {
            let start = Instant::now();
            action();
            println!(
                "{label},{sample},{:.3}",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    };
    measure("store_list", &mut || {
        std::hint::black_box(ctx.store.list().unwrap());
    });
    measure("events", &mut || {
        std::hint::black_box(ctx.store.events().unwrap());
    });
    measure("resource_sample", &mut || {
        std::hint::black_box(ctx.resource_probe.sample(&ctx.store).unwrap());
    });
    let tasks = ctx.store.list().unwrap();
    measure("finishes", &mut || {
        std::hint::black_box(corral_ops::finishes(&ctx.store, &tasks).unwrap());
    });
    measure("queue", &mut || {
        std::hint::black_box(corral_ops::dispatch_queue(&ctx).unwrap());
    });
    let mut board = crate::board::BoardView::new(&ctx).unwrap();
    measure("board_refresh", &mut || board.refresh(&ctx));
    let mut app = App::new(ctx, Mode::Board).unwrap();
    measure("app_reload", &mut || app.reload());
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 45)).unwrap();
    measure("draw", &mut || {
        terminal.draw(|frame| app.draw(frame)).unwrap();
    });
}

#[test]
#[ignore = "requires CORRAL_LATENCY_FIXTURE from scripts/qa/board-latency.mjs"]
fn profile_search_stages() {
    let root = PathBuf::from(std::env::var_os("CORRAL_LATENCY_FIXTURE").unwrap());
    let store = Store::open(&root).unwrap();
    let start = Instant::now();
    let data = corral_ops::SearchData::load(&store).unwrap();
    println!("search_load,{:.3}", start.elapsed().as_secs_f64() * 1000.0);
    for text in ["s", "st", "status", "status:done"] {
        let query = corral_core::query::Query::parse(text, jiff::Timestamp::now()).unwrap();
        for sample in 0..5 {
            let start = Instant::now();
            std::hint::black_box(data.ranked(&query));
            println!(
                "rank_{text},{sample},{:.3}",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}
