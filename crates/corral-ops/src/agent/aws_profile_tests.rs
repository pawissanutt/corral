use super::*;

const SECRET: &str = "CANARY_SECRET_MUST_NOT_APPEAR";
const STATIC: &str = "aws_access_key_id = CANARY_ACCESS_KEY\naws_secret_access_key = CANARY_SECRET_MUST_NOT_APPEAR\n";
const SESSION_HASH: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
const LEGACY_HASH: &str = "e8be5486177c5b5392bd9aa76563515b29358e6e";

fn fixture(config: &str, credentials: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".aws/sso/cache")).unwrap();
    std::fs::write(home.path().join(".aws/config"), config).unwrap();
    std::fs::write(home.path().join(".aws/credentials"), credentials).unwrap();
    home
}

fn session_config() -> &'static str {
    "[profile dev]\nregion = us-west-2\nsso_session = test\nsso_account_id = 123456789012\n\
     sso_role_name = Developer\n[sso-session test]\nsso_region = us-east-1\n\
     sso_start_url = https://example.awsapps.com/start\n"
}

fn cache(home: &Path, hash: &str, expiry: &str) {
    std::fs::write(
        home.join(".aws/sso/cache").join(format!("{hash}.json")),
        format!(r#"{{"accessToken":"{SECRET}","expiresAt":"{expiry}"}}"#),
    )
    .unwrap();
}

fn now() -> Timestamp {
    "2026-10-29T12:00:00Z".parse().unwrap()
}

type Snapshot = Vec<(std::path::PathBuf, Vec<u8>, std::time::SystemTime)>;

fn snapshot(root: &Path) -> Snapshot {
    fn visit(root: &Path, path: &Path, files: &mut Snapshot) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(&path).unwrap(),
                    std::fs::metadata(&path).unwrap().modified().unwrap(),
                ));
            }
        }
    }
    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[test]
fn static_keys_in_either_file_return_only_metadata_without_aws_writes() {
    // Given both supported static-key locations, including an optional session token.
    for (config, credentials) in [
        (
            "[profile dev]\nregion = us-west-2\n".to_owned(),
            format!("[dev]\n{STATIC}aws_session_token = {SECRET}\n"),
        ),
        (
            format!("[profile dev]\nregion = us-west-2\n{STATIC}"),
            String::new(),
        ),
    ] {
        let home = fixture(&config, &credentials);
        let before = snapshot(home.path());
        // When the selected profile is inspected.
        let status = inspect_profile(home.path(), "dev").unwrap();
        // Then only non-secret metadata escapes and every AWS byte and mtime is unchanged.
        assert_eq!(status.configured_region.as_deref(), Some("us-west-2"));
        status.check_ready(now()).unwrap();
        let output = format!("{status:?} {}", status.description());
        assert!(!output.contains(SECRET));
        assert!(!output.contains("CANARY_ACCESS_KEY"));
        assert_eq!(snapshot(home.path()), before);
    }
}

#[test]
fn default_is_a_named_profile_not_an_ambient_fallback() {
    // Given only a default profile in both files.
    let home = fixture(
        "[default]\nregion = eu-west-1\n",
        &format!("[default]\n{STATIC}"),
    );
    // When an explicitly selected name is inspected.
    let missing = inspect_profile(home.path(), "dev").unwrap_err();
    // Then it cannot fall back to default; selecting default explicitly remains supported.
    assert!(missing.to_string().starts_with("credentials_unavailable:"));
    inspect_profile(home.path(), "default")
        .unwrap()
        .check_ready(now())
        .unwrap();
}

#[test]
fn credentials_file_overrides_static_config_and_config_region_is_retained() {
    // Given an incomplete config key overridden by a complete credentials entry.
    let home = fixture(
        "[profile dev]\nregion = ap-southeast-2\naws_access_key_id = \n",
        &format!("[dev]\n{STATIC}"),
    );
    // When inspected.
    let status = inspect_profile(home.path(), "dev").unwrap();
    // Then it is ready with the profile's region.
    status.check_ready(now()).unwrap();
    assert_eq!(status.configured_region.as_deref(), Some("ap-southeast-2"));
}

fn role_config(steps: usize) -> String {
    let mut config = String::new();
    for index in 0..steps {
        config.push_str(&format!(
            "[profile role{index}]\nrole_arn = arn:aws:iam::123456789012:role/test\n\
             source_profile = role{}\nexternal_id = {SECRET}\n\
             role_session_name = fixture\nduration_seconds = 3600\n",
            index + 1
        ));
    }
    config.push_str(&format!("[profile role{steps}]\n{STATIC}"));
    config
}

#[test]
fn four_assume_role_steps_are_supported_without_exposing_role_values() {
    // Given the maximum supported chain rooted in static keys.
    let home = fixture(&role_config(4), "");
    // When the root is inspected.
    let status = inspect_profile(home.path(), "role0").unwrap();
    // Then all four steps are recognized without any secret values.
    assert_eq!(status.role_steps, 4);
    status.check_ready(now()).unwrap();
    assert!(!format!("{status:?} {}", status.description()).contains(SECRET));
}

#[test]
fn five_assume_role_steps_and_cycles_are_refused() {
    // Given a too-deep chain and a cyclic chain.
    for config in [
        role_config(5),
        "[profile role0]\nrole_arn = fixture\nsource_profile = role1\n\
         [profile role1]\nrole_arn = fixture\nsource_profile = role0\n"
            .to_owned(),
    ] {
        let home = fixture(&config, "");
        let before = snapshot(home.path());
        // When inspected.
        let error = inspect_profile(home.path(), "role0").unwrap_err();
        // Then no credential metadata is accepted and AWS files remain unchanged.
        assert!(error.to_string().starts_with("credentials_unavailable:"));
        assert_eq!(snapshot(home.path()), before);
    }
}

#[test]
fn unsupported_mechanisms_are_refused_even_in_a_source_profile() {
    // Given a source containing each forbidden mechanism, even alongside usable static keys.
    for key in [
        "credential_process",
        "web_identity_token_file",
        "credential_source",
        "mfa_serial",
        "login_session",
    ] {
        let home = fixture(
            &format!(
                "[profile dev]\nrole_arn = fixture\nsource_profile = base\n\
                 [profile base]\n{STATIC}{key} = {SECRET}\n"
            ),
            "",
        );
        let before = snapshot(home.path());
        // When the selected chain is inspected.
        let error = inspect_profile(home.path(), "dev").unwrap_err();
        // Then it identifies the key, never its value.
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains(key));
        assert!(!diagnostic.contains(SECRET));
        assert_eq!(snapshot(home.path()), before);
    }
}

#[test]
fn endpoint_overrides_and_noncommercial_regions_fail_closed() {
    // Given direct, nested-service and partition endpoint changes.
    for setting in [
        "endpoint_url = https://example.invalid",
        "use_fips_endpoint = true",
        "use_dualstack_endpoint = true",
        "region = cn-north-1",
        "region = us-gov-west-1",
        "services = custom\n[services custom]\nbedrock-runtime =\n  endpoint_url = https://example.invalid",
    ] {
        let home = fixture(&format!("[profile dev]\n{STATIC}{setting}\n"), "");
        // When inspected.
        let error = inspect_profile(home.path(), "dev").unwrap_err();
        // Then no endpoint override is usable.
        assert!(
            error.to_string().starts_with("network_refused:")
                || error.to_string().starts_with("credentials_unavailable:")
        );
    }
}

#[test]
fn disabled_endpoint_flags_are_allowed() {
    // Given flags explicitly disabled by the AWS config.
    let home = fixture(
        &format!(
            "[profile dev]\n{STATIC}use_fips_endpoint = false\nuse_dualstack_endpoint = false\n"
        ),
        "",
    );
    // When inspected.
    let status = inspect_profile(home.path(), "dev").unwrap();
    // Then the ordinary static profile stays ready.
    status.check_ready(now()).unwrap();
}

#[test]
fn session_sso_uses_sha1_session_filename_and_reads_only_expiry() {
    // Given a cache without startUrl under the AWS CLI's SHA-1(session name) filename.
    let home = fixture(session_config(), "");
    cache(home.path(), SESSION_HASH, "2026-10-29T13:00:00Z");
    let before = snapshot(home.path());
    // When the profile is inspected offline.
    let status = inspect_profile(home.path(), "dev").unwrap();
    // Then it recognizes the token expiry without retaining token values or writing files.
    status.check_ready(now()).unwrap();
    assert!(matches!(
        status.mechanism,
        Mechanism::Sso { expires_at: Some(expiry), .. }
            if expiry == "2026-10-29T13:00:00Z".parse::<Timestamp>().unwrap()
    ));
    assert!(!format!("{status:?} {}", status.description()).contains(SECRET));
    assert_eq!(snapshot(home.path()), before);
}

#[test]
fn legacy_sso_uses_sha1_start_url_filename_and_accepts_cli_utc_expiry() {
    // Given a legacy SSO profile and the AWS CLI UTC timestamp representation.
    let home = fixture(
        "[profile dev]\nsso_start_url = https://example.awsapps.com/start\n\
         sso_region = us-east-1\nsso_account_id = 123456789012\nsso_role_name = Developer\n",
        "",
    );
    cache(home.path(), LEGACY_HASH, "2026-10-29T13:00:00UTC");
    let before = snapshot(home.path());
    // When inspected.
    let status = inspect_profile(home.path(), "dev").unwrap();
    // Then the cache is ready and byte/mtime-identical.
    status.check_ready(now()).unwrap();
    assert_eq!(snapshot(home.path()), before);
}

#[test]
fn sso_missing_invalid_and_exactly_expired_tokens_need_owner_login_without_writes() {
    // Given all unavailable token states, including the exact hard-expiry boundary.
    for expiry in [None, Some(SECRET), Some("2026-10-29T12:00:00Z")] {
        let home = fixture(session_config(), "");
        if let Some(expiry) = expiry {
            cache(home.path(), SESSION_HASH, expiry);
        }
        let before = snapshot(home.path());
        // When readiness is checked at a fixed instant.
        let status = inspect_profile(home.path(), "dev").unwrap();
        let error = status.check_ready(now()).unwrap_err();
        // Then the diagnostic is non-secret and the offline reader never refreshes anything.
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("aws sso login --profile dev"));
        assert!(!diagnostic.contains(SECRET));
        assert!(!format!("{status:?} {}", status.description()).contains(SECRET));
        assert_eq!(snapshot(home.path()), before);
    }
}

#[test]
fn assume_role_from_sso_reports_the_source_profile_for_refresh() {
    // Given a role rooted in SSO with no token.
    let config = format!(
        "{}[profile role]\nregion = eu-west-1\nrole_arn = fixture\nsource_profile = dev\n",
        session_config()
    );
    let home = fixture(&config, "");
    // When the role readiness is checked.
    let status = inspect_profile(home.path(), "role").unwrap();
    let error = status.check_ready(now()).unwrap_err();
    // Then the user is sent to the source profile, while region remains the selected role's.
    assert_eq!(status.configured_region.as_deref(), Some("eu-west-1"));
    assert_eq!(status.role_steps, 1);
    assert!(error.to_string().contains("aws sso login --profile dev"));
}

#[test]
fn malformed_secret_input_never_reaches_diagnostics() {
    // Given malformed INI and token JSON carrying secret canaries.
    let home = fixture(&format!("[profile dev]\n{SECRET}\n"), "");
    let error = inspect_profile(home.path(), "dev").unwrap_err();
    assert!(!format!("{error:#}").contains(SECRET));
    let home = fixture(session_config(), "");
    std::fs::write(
        home.path()
            .join(".aws/sso/cache")
            .join(format!("{SESSION_HASH}.json")),
        format!("{{{SECRET}"),
    )
    .unwrap();
    // When malformed cache data is inspected and readiness checked.
    let status = inspect_profile(home.path(), "dev").unwrap();
    let error = status.check_ready(now()).unwrap_err();
    // Then neither status nor errors contain token JSON.
    assert!(!format!("{status:?} {error:#}").contains(SECRET));
}

#[test]
fn invalid_or_missing_profile_and_invalid_region_write_no_binding() {
    // Given no usable selected profile or an invalid bound region.
    for (profile, region, config) in [
        ("dev", "us-west-2", String::new()),
        ("../dev", "us-west-2", format!("[profile dev]\n{STATIC}")),
        ("dev", "cn-north-1", format!("[profile dev]\n{STATIC}")),
        ("dev", "us-gov-east-1", format!("[profile dev]\n{STATIC}")),
        (
            "dev",
            "us-west-2",
            "[profile dev]\naws_access_key_id = key\n".to_owned(),
        ),
        (
            "dev",
            "us-west-2",
            format!("[profile dev]\n{STATIC}credential_process = {SECRET}\n"),
        ),
    ] {
        let home = fixture(&config, "");
        let root = tempfile::tempdir().unwrap();
        let before = snapshot(home.path());
        // When login validates before any auth write.
        let error = login_in(root.path(), home.path(), profile, region).unwrap_err();
        // Then neither a binding nor its auth directory was created.
        assert!(!auth_dir(root.path()).exists());
        assert!(!format!("{error:#}").contains(SECRET));
        assert_eq!(snapshot(home.path()), before);
    }
}

#[test]
fn nonterminal_login_requires_both_flags_and_never_prompts() {
    // Given a configured static profile and nonterminal input that must never be consumed.
    let home = fixture(&format!("[profile dev]\nregion = us-west-2\n{STATIC}"), "");
    let root = tempfile::tempdir().unwrap();
    for (profile, region) in [(None, None), (Some("dev"), None), (None, Some("us-west-2"))] {
        let mut input = std::io::Cursor::new(b"should not be read\n");
        let mut output = Vec::new();
        // When flags are incomplete.
        let error = login_with_io(
            root.path(),
            home.path(),
            profile,
            region,
            false,
            &mut input,
            &mut output,
        )
        .unwrap_err();
        // Then it fails with usage_error without reading, prompting or writing.
        assert!(error.to_string().starts_with("usage_error:"));
        assert_eq!(input.position(), 0);
        assert!(output.is_empty());
        assert!(!auth_dir(root.path()).exists());
    }
}

#[cfg(unix)]
#[test]
fn explicit_login_writes_exact_private_binding_and_preserves_aws_files() {
    use std::os::unix::fs::PermissionsExt;
    // Given supported static credentials in fixture files.
    let home = fixture(
        "[profile dev]\nregion = us-west-2\n",
        &format!("[dev]\n{STATIC}"),
    );
    let root = tempfile::tempdir().unwrap();
    let before = snapshot(home.path());
    // When login runs entirely in Rust.
    login_in(root.path(), home.path(), "dev", "us-west-2").unwrap();
    // Then the binding contains exactly the three public fields and is private.
    let path = auth_dir(root.path()).join(BINDING_FILE);
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "type": "aws_profile", "profile": "dev", "region": "us-west-2"
        })
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(auth_dir(root.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        read_binding(root.path()).unwrap(),
        Some(AwsBinding {
            profile: "dev".to_owned(),
            region: "us-west-2".to_owned()
        })
    );
    assert_eq!(snapshot(home.path()), before);
}

#[cfg(unix)]
#[test]
fn terminal_line_prompts_default_to_the_selected_profiles_region() {
    // Given terminal responses selecting dev and accepting its configured region.
    let home = fixture(&format!("[profile dev]\nregion = us-west-2\n{STATIC}"), "");
    let root = tempfile::tempdir().unwrap();
    let mut input = std::io::Cursor::new(b"dev\n\n");
    let mut output = Vec::new();
    // When line-prompt login runs.
    login_with_io(
        root.path(),
        home.path(),
        None,
        None,
        true,
        &mut input,
        &mut output,
    )
    .unwrap();
    // Then the selected region is persisted and prompts contain no terminal control sequences.
    assert_eq!(
        read_binding(root.path()).unwrap().unwrap().region,
        "us-west-2"
    );
    assert!(!output.contains(&0x1b));
    assert!(!String::from_utf8(output).unwrap().contains(SECRET));
}

#[cfg(unix)]
#[test]
fn binding_reader_rejects_wrong_wrapper_extra_fields_permissions_and_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    // Given bindings whose bytes or metadata violate the exact storage contract.
    for bytes in [
        format!(r#"{{"type":"{SECRET}","profile":"dev","region":"us-west-2"}}"#),
        r#"{"type":{"aws_profile":null},"profile":"dev","region":"us-west-2"}"#.to_owned(),
        format!(
            r#"{{"type":"aws_profile","profile":"dev","region":"us-west-2","token":"{SECRET}"}}"#
        ),
        r#"{"type":"aws_profile","profile":"../bad","region":"us-west-2"}"#.to_owned(),
        r#"{"type":"aws_profile","profile":"dev","region":"cn-north-1"}"#.to_owned(),
    ] {
        let root = tempfile::tempdir().unwrap();
        ensure_auth_dir(&auth_dir(root.path())).unwrap();
        let path = auth_dir(root.path()).join(BINDING_FILE);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        // When read.
        let error = read_binding(root.path()).unwrap_err();
        // Then no invalid binding escapes, including secret values from serde errors.
        assert!(!format!("{error:#}").contains(SECRET));
    }
    let root = tempfile::tempdir().unwrap();
    assert!(read_binding(root.path()).unwrap().is_none());
    ensure_auth_dir(&auth_dir(root.path())).unwrap();
    let path = auth_dir(root.path()).join(BINDING_FILE);
    std::fs::write(
        &path,
        r#"{"type":"aws_profile","profile":"dev","region":"us-west-2"}"#,
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_binding(root.path()).is_err());
    std::fs::remove_file(&path).unwrap();
    let target = root.path().join("target");
    std::fs::write(&target, SECRET).unwrap();
    symlink(&target, &path).unwrap();
    assert!(read_binding(root.path()).is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), SECRET);
}

#[test]
fn aws_options_for_oauth_providers_are_usage_errors_before_login() {
    // Given AWS flags on a subscription provider.
    let root = tempfile::tempdir().unwrap();
    // When the existing login API's extended entry point is called.
    let error = super::super::credentials::login_with_aws(
        root.path(),
        Some("anthropic"),
        &super::super::credentials::AwsLoginOptions {
            profile: Some("dev"),
            region: Some("us-west-2"),
        },
    )
    .unwrap_err();
    // Then it refuses before any OAuth or AWS login starts.
    assert!(error.to_string().starts_with("usage_error:"));
}
