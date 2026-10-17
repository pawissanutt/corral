//! Opt-in stage profiling against the disposable live-QA workload, not a timing gate.

use std::path::PathBuf;
use std::time::Instant;

use corral_core::Store;
use corral_ops::Ctx;
use ratatui::Terminal;

use super::App;
use crate::Mode;
use crate::View;

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
