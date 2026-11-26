use std::time::Duration;

use nsh_core::platform::{Idiom, Os};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;

use super::*;
use crate::model::Form;
use crate::model::settings::pi_hosted_tool_models::request;

#[component]
fn ModelPickerFixture() -> impl IntoElement {
    let app = AppState::install(Form::Desktop);
    let tool = Tool::Image;
    let index = tool as usize;
    let route = *tool
        .routes()
        .iter()
        .find(|route| route.0 == "openai")
        .unwrap();
    let key = Id::from("fixture-models");
    let mut editor = Editor::default();
    editor.models[index].want(request(tool, route.0));
    editor.models[index].sent(Some((key.clone(), true)));
    let editor = signal(editor);
    view().child(model(app, editor, tool, route)).child(
        view()
            .test_id("release-listing")
            .on_press(move || {
                let reply = Operation {
                    method: "config.pi_hosted_tool_models".into(),
                    value: Some(json!({"status":"live","models":["gpt-image-fixture"]})),
                    loading: false,
                    error: None,
                    refusal: None,
                };
                editor.update(|editor| editor.models[index].settle(&key, &reply));
            })
            .child(text("Release listing")),
    )
}

#[test]
fn pi_hosted_model_discovery_preserves_keyboard_focus() {
    let mut config = EngineConfig::new(Root::of::<ModelPickerFixture>());
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(config).unwrap();
    let field = "id:field-pi-hosted-tools-image-gen-model";
    let picker = "id:btn-pi-hosted-tools-image-gen-model";
    let option = "id:seg-pi-hosted-tools-image-gen-model-gpt-image-fixture";
    app.wait_for(field).unwrap();
    app.press_key("tab").unwrap();
    // Release the pending provider reply while the fallback input has the keyboard.
    app.press("id:release-listing").unwrap();
    app.wait_for(picker).unwrap();
    app.wait_for(field).unwrap();
    app.type_text(field, "manual-id").unwrap();
    app.press_key("shift+tab").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for(option).unwrap();
    app.press_key("down").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for_gone(field, Duration::from_secs(2)).unwrap();
    app.wait_for_gone(option, Duration::from_secs(2)).unwrap();
    // The menu should have restored its trigger's focus; Enter reopens it without tabbing.
    app.press_key("enter").unwrap();
    app.wait_for(option).unwrap();
}
