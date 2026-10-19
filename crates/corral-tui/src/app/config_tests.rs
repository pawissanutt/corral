//! Config lifecycle journeys reuse the settings fixture without live discovery or herdr.

use corral_core::Event;
use corral_ops::capture_messages;
use ratatui::layout::Rect;

use super::{Fixture, draw_at, mouse, position};
use crate::app::App;
use crate::board::tests::code;
use crate::choice::RunChoice;
use crate::{Action, Hit, HitMap, Mode, Tone};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend};

const SIZES: [(u16, u16); 2] = [(80, 24), (160, 45)];

fn events(app: &App, kind: &str) -> Vec<Event> {
    app.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == kind)
        .collect()
}

fn click_label(app: &mut App, size: (u16, u16), row: &str, label: &str) {
    let screen = draw_at(app, size.0, size.1);
    let at = position(&screen, row, label);
    mouse(app, MouseEventKind::Down(MouseButton::Left), at);
    mouse(app, MouseEventKind::Up(MouseButton::Left), at);
}

#[test]
fn config_lifecycle_settings_save_has_keyboard_mouse_event_parity() {
    for size in SIZES {
        let mut details = Vec::new();
        for by_mouse in [false, true] {
            let fx = Fixture::new();
            let mut app = fx.app();
            let baseline = events(&app, "config_changed").len();
            if by_mouse {
                click_label(&mut app, size, "Default profile", " claude ");
                click_label(&mut app, size, "s save", "s save");
            } else {
                for key in [
                    KeyCode::Down,
                    KeyCode::Right,
                    KeyCode::Right,
                    KeyCode::Char('s'),
                ] {
                    app.handle_key(code(key));
                }
            }
            assert_eq!(fx.config().defaults.profile.as_deref(), Some("claude"));
            let saved = events(&app, "config_changed");
            assert_eq!(saved.len(), baseline + 1);
            let detail = saved.last().unwrap().detail.clone();
            assert_eq!(detail["source"], "settings_ui");
            let changes = detail["changes"].as_array().unwrap();
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0]["key"], "defaults.profile");
            assert_eq!(changes[0]["old"], "omp");
            assert_eq!(changes[0]["new"], "claude");
            details.push(detail["changes"].clone());
            let screen = draw_at(&mut app, size.0, size.1);
            assert!(screen.contains("Settings saved"), "{screen}");
            assert!(
                screen.contains("settings") && screen.contains("s save"),
                "{screen}"
            );
            app.handle_key(code(KeyCode::F(8)));
            app.handle_key(code(KeyCode::Char('s')));
            assert_eq!(events(&app, "config_changed").len(), saved.len());
        }
        assert_eq!(
            details[0], details[1],
            "input methods produce the same semantic diff"
        );
    }
}

#[test]
fn config_lifecycle_saved_profile_uses_settings_source() {
    for (width, height) in SIZES {
        let fx = Fixture::new();
        let app = fx.app();
        let baseline = events(&app, "config_changed").len();
        let mut choice = RunChoice::new(&fx.ctx.store.config, "claude", None, Some("high".into()));
        choice.on_key(&code(KeyCode::Char('p')), &fx.ctx);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|frame| {
                choice.draw(frame, Rect::new(0, 0, width, 3), 10, &mut hits);
                choice.draw_popups(frame, frame.area(), &mut hits);
            })
            .unwrap();
        let text = crate::board::tests::buffer_text(terminal.backend().buffer());
        assert!(text.contains("save as profile"), "{text}");
        let Some(Action::Toast(toast)) = choice.on_key(&code(KeyCode::Enter), &fx.ctx) else {
            panic!("profile save did not return a toast");
        };
        assert_eq!(toast.tone, Tone::Good);
        assert_eq!(
            fx.config().profiles["claude-high"].effort.as_deref(),
            Some("high")
        );
        let saved = events(&app, "config_changed");
        assert_eq!(saved.len(), baseline + 1);
        assert_eq!(saved.last().unwrap().detail["source"], "settings_ui");
        assert!(
            saved.last().unwrap().detail["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["key"] == "profiles.claude-high.effort"
                    && change["old"].is_null()
                    && change["new"] == "high")
        );
    }
}

#[test]
fn config_lifecycle_invalid_reload_retains_config_and_shared_message_history() {
    for size in SIZES {
        for by_mouse in [false, true] {
            let fx = Fixture::new();
            let mut app = App::new(fx.ctx.clone(), Mode::Board).unwrap();
            let before = app.ctx.store.config.clone();
            let baseline = draw_at(&mut app, size.0, size.1);
            let _capture = capture_messages(app.message_tx.clone());
            std::fs::write(
                app.ctx.store.config_path(),
                "[limits]\nrunning = \"PRIVATE_CONFIG_VALUE\"\n",
            )
            .unwrap();
            assert!(!app.ctx.reload_config().unwrap());
            assert_eq!(app.ctx.store.config, before);
            app.reload();
            app.drain_messages();
            assert_eq!(app.ctx.store.config, before);
            assert_eq!(app.messages.len(), 1);
            assert_eq!(app.toasts.len(), 1, "invalid reload must not toast twice");
            assert_eq!(app.toasts[0].0.title, "Config not reloaded");
            assert!(app.toasts[0].0.task.is_none());
            assert!(!app.toasts[0].0.body.contains("PRIVATE_CONFIG_VALUE"));
            let invalid = events(&app, "config_invalid");
            assert_eq!(invalid.len(), 1);
            assert!(
                !invalid[0]
                    .detail
                    .to_string()
                    .contains("PRIVATE_CONFIG_VALUE")
            );
            let screen = draw_at(&mut app, size.0, size.1);
            assert!(screen.contains("Config not reloaded"), "{screen}");
            if by_mouse {
                let (rect, _) = app
                    .hits
                    .iter()
                    .find(|(_, hit)| *hit == Hit::Toast(0))
                    .unwrap();
                let at = (rect.x, rect.y);
                mouse(&mut app, MouseEventKind::Down(MouseButton::Left), at);
            } else {
                app.handle_key(code(KeyCode::F(8)));
            }
            assert!(app.toasts.is_empty());
            let rejected = draw_at(&mut app, size.0, size.1);
            assert_eq!(rejected.lines().next(), baseline.lines().next());
            assert!(rejected.contains("policy unavailable;"), "{rejected}");
            assert!(!rejected.contains("PRIVATE_CONFIG_VALUE"), "{rejected}");
            if by_mouse {
                click_label(&mut app, size, "M messages", "M messages");
            } else {
                app.handle_key(code(KeyCode::Char('M')));
            }
            let screen = draw_at(&mut app, size.0, size.1);
            assert!(
                screen.contains("Board messages") && screen.contains("Config not reloaded"),
                "{screen}"
            );
            assert!(!screen.contains("PRIVATE_CONFIG_VALUE"), "{screen}");
            assert_eq!(app.messages.len(), 1, "dismissal retains message history");
        }
    }
}
