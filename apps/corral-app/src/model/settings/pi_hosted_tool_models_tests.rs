use super::*;
use serde_json::{Value, json};

fn reply(value: Value) -> Operation {
    Operation {
        method: "test".into(),
        loading: false,
        value: Some(value),
        error: None,
        refusal: None,
    }
}

fn models(status: &str, models: &[&str]) -> Operation {
    reply(json!({"status": status, "models": models}))
}

/// The editor once `config.pi_hosted_tools` answered `selector` for `tool`.
fn loaded(tool: Tool, selector: &str) -> Editor {
    let mut editor = Editor::default();
    editor.sent(Id::from("read"), false, true);
    let key = match tool {
        Tool::Search => "web_search",
        Tool::Image => "image_gen",
    };
    let mut saved = json!({"revision": "r1", "web_search": null, "image_gen": null});
    saved[key] = json!(selector);
    editor.settle(reply(saved));
    editor
}

fn route(tool: Tool, name: &str) -> usize {
    tool.routes()
        .iter()
        .position(|route| route.0 == name)
        .unwrap()
}

/// Sends what the shown route wants under `key`, as the screen does.
fn ask(editor: &mut Editor, tool: Tool, key: &str) -> PiHostedToolModelsRequest {
    let wanted = editor.wanted(tool).unwrap();
    editor.models[tool as usize].sent(Some((Id::from(key), true)));
    wanted
}

#[test]
fn late_list_for_an_earlier_route_is_dropped_even_after_returning_to_it() {
    let mut editor = loaded(Tool::Search, "openai-codex/m");
    let search = Tool::Search as usize;
    let first = ask(&mut editor, Tool::Search, "a1");
    assert_eq!(first.provider, PiHostedToolProvider::OpenAiCodex);
    assert_eq!(editor.wanted(Tool::Search), None);
    editor.draft[search].route = route(Tool::Search, "anthropic");
    ask(&mut editor, Tool::Search, "b");
    editor.draft[search].route = route(Tool::Search, "openai-codex");
    assert_eq!(ask(&mut editor, Tool::Search, "a2"), first);
    // A's first reply and B's reply land after A was asked again.
    for key in ["a1", "b"] {
        editor.models[search].settle(&Id::from(key), &models("live", &["stale"]));
        assert_eq!(editor.models[search].listed(), None);
        assert!(editor.models[search].pending().is_some());
    }
    editor.models[search].settle(&Id::from("a2"), &models("live", &["fresh"]));
    assert_eq!(editor.models[search].listed().unwrap(), ["fresh"]);
}

#[test]
fn listed_pick_saves_and_a_saved_id_the_provider_omits_stays_chosen() {
    let mut editor = loaded(Tool::Image, "openai/kept-id");
    let image = Tool::Image as usize;
    let request = ask(&mut editor, Tool::Image, "i1");
    assert_eq!(request.tool, PiHostedTool::ImageGen);
    assert!(editor.models[image].typing());
    editor.models[image].settle(&Id::from("i1"), &models("live", &["gpt-image-1"]));
    assert!(!editor.models[image].typing());
    assert_eq!(editor.models[image].listed().unwrap(), ["gpt-image-1"]);
    assert!(
        editor.models[image]
            .selection(&editor.draft[image].model)
            .starts_with("kept-id")
    );
    assert!(editor.change().is_none());
    editor.pick(Tool::Image, "gpt-image-1".into());
    assert_eq!(
        editor.change().unwrap().image_gen.as_deref(),
        Some("openai/gpt-image-1")
    );
}

#[test]
fn typed_id_survives_a_missing_login_save_and_reload() {
    let mut editor = loaded(Tool::Search, "anthropic-api/old");
    let search = Tool::Search as usize;
    ask(&mut editor, Tool::Search, "s1");
    editor.models[search].settle(&Id::from("s1"), &models("missing_login", &[]));
    assert!(editor.models[search].typing());
    assert!(editor.models[search].note(Tool::Search, "anthropic-api").1);
    editor.type_model(Tool::Search, "typed-id".into());
    let change = editor.change().unwrap();
    assert_eq!(change.web_search.as_deref(), Some("anthropic-api/typed-id"));
    editor.sent(Id::from("save"), true, true);
    editor.settle(reply(
        json!({"revision": "r2", "web_search": "anthropic-api/typed-id", "image_gen": null}),
    ));
    editor.sent(Id::from("reload"), false, true);
    editor.settle(reply(
        json!({"revision": "r2", "web_search": "anthropic-api/typed-id", "image_gen": null}),
    ));
    // The same route keeps its list and the owner's choice to type.
    assert_eq!(editor.wanted(Tool::Search), None);
    assert!(editor.models[search].manual);
    assert_eq!(editor.draft[search].model, "typed-id");
    assert!(editor.change().is_none());
    // A connection without listing leaves the typed field, never a list.
    let mut fresh = loaded(Tool::Search, "anthropic-api/typed-id");
    fresh.wanted(Tool::Search).unwrap();
    fresh.models[search].sent(None);
    assert!(fresh.models[search].typing());
    assert_eq!(fresh.models[search].listed(), None);
}
