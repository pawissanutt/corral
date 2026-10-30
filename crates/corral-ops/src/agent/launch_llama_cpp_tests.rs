//! Launch payload checks for the local llama.cpp provider (T-578), kept beside `launch.rs`.
use super::*;

#[test]
fn launch_payload_includes_local_settings_only_when_effective() {
    // Given recorded settings, including a disabled snapshot with stale settings.
    for enabled in [false, true] {
        let mut bootstrap: Bootstrap = serde_json::from_str(include_str!(
            "../../../../packages/corral-pi/test/fixtures/llama-bootstrap.json"
        ))
        .unwrap();
        let snapshot = bootstrap.capabilities.as_mut().unwrap();
        let feature = snapshot.features.get_mut("llama_cpp.enabled").unwrap();
        feature.enabled = enabled;
        let local = snapshot.llama_cpp.clone().unwrap();
        bootstrap.provider = host_provider(Some("llama.cpp"), "openai/x");
        let ready = Ready {
            assets: "/fixture/assets".into(),
            bun: "/fixture/bun".into(),
            bun_version: "1.4.2".into(),
            generation_dir: "/fixture/g1".into(),
        };
        let secrets = Secrets::generate().unwrap();
        // When serialized, then only effective local settings cross the launch boundary.
        let message: serde_json::Value =
            serde_json::from_slice(&launch_message(&bootstrap, &ready, &secrets).unwrap()).unwrap();
        assert_eq!(
            message["capabilities"].get("llama_cpp"),
            enabled.then(|| json!(local)).as_ref()
        );
        assert_eq!(message["provider"], "llama.cpp");
        assert!(message["aws"].is_null());
        assert!(super::super::credentials::provider_for_vendor("llama.cpp").is_none());
    }
}
