use corral_core::{ModelFamily, model_family, model_key};

const BEDROCK: Option<&str> = Some("amazon-bedrock");

fn family(vendor: Option<&str>, model: Option<&str>) -> Option<String> {
    model_family(vendor, model).map(|f| f.to_string())
}

#[test]
fn bedrock_ids_map_to_their_publisher_and_other_vendors_to_themselves() {
    for (vendor, model, expected) in [
        (
            BEDROCK,
            Some("us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
            Some("anthropic"),
        ),
        (
            BEDROCK,
            Some("anthropic.claude-3-haiku-20240307-v1:0"),
            Some("anthropic"),
        ),
        (
            BEDROCK,
            Some("global.anthropic.claude-opus-4-1"),
            Some("anthropic"),
        ),
        (
            BEDROCK,
            Some("amazon-bedrock/us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
            Some("anthropic"),
        ),
        (
            BEDROCK,
            Some("amazon-bedrock/anthropic.claude-3-haiku-20240307-v1:0"),
            Some("anthropic"),
        ),
        (BEDROCK, Some("openai.gpt-oss-120b-1:0"), Some("openai")),
        (
            BEDROCK,
            Some("us.meta.llama3-3-70b-instruct-v1:0"),
            Some("meta"),
        ),
        (
            BEDROCK,
            Some("meta.llama3-3-70b-instruct-v1:0"),
            Some("meta"),
        ),
        (BEDROCK, Some("amazon.nova-pro-v1:0"), Some("amazon")),
        (BEDROCK, Some("us.amazon.nova-pro-v1:0"), Some("amazon")),
        (
            BEDROCK,
            Some(
                "arn:aws:bedrock:us-east-1:123456789012:inference-profile/us.anthropic.claude-sonnet-4",
            ),
            None,
        ),
        (BEDROCK, Some(""), None),
        (BEDROCK, Some("us."), None),
        (BEDROCK, None, None),
        (
            Some("anthropic"),
            Some("us.meta.llama3-3-70b-instruct-v1:0"),
            Some("anthropic"),
        ),
        (
            Some("openai"),
            Some("anthropic/claude-opus-4-1"),
            Some("openai"),
        ),
        (Some("google"), None, Some("google")),
        (
            Some(" "),
            Some("anthropic.claude-3-haiku-20240307-v1:0"),
            None,
        ),
        (None, Some("anthropic.claude-3-haiku-20240307-v1:0"), None),
    ] {
        assert_eq!(
            family(vendor, model).as_deref(),
            expected,
            "{vendor:?} {model:?}"
        );
    }
}

#[test]
fn family_equality_ignores_the_routing_vendor() {
    let bedrock = model_family(
        BEDROCK,
        Some("us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
    );
    let direct = model_family(Some("anthropic"), Some("anthropic/claude-sonnet-4-5"));
    assert_eq!(bedrock, direct);
    assert_eq!(direct, Some(ModelFamily::from("anthropic")));
}

#[test]
fn model_key_distinguishes_bedrock_models_sharing_a_version_suffix() {
    let claude = model_key(BEDROCK, "us.anthropic.claude-sonnet-4-5-20250929-v1:0");
    let llama = model_key(BEDROCK, "us.meta.llama3-3-70b-instruct-v1:0");
    assert_eq!(claude, "claude-sonnet-4-5-20250929");
    assert_eq!(llama, "llama3-3-70b-instruct");
    assert_ne!(claude, llama);
    // The last-segment rule other vendors keep would see both IDs as "0".
    assert_eq!(
        model_key(None, "us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
        "0"
    );
    assert_eq!(
        model_key(BEDROCK, "amazon-bedrock/global.anthropic.claude-opus-4-1"),
        "claude-opus-4-1"
    );
    assert_eq!(
        model_key(Some("openai"), "openai/GPT-6.1-Sol"),
        "gpt-6.1-sol"
    );
}
