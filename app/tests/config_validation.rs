use hft_app::{live::config::AppConfig, ports::RunMode};

const PUBLIC_ONLY: &str = r#"
public_stream_url = "wss://example.test/ws"
api_url = "https://example.test"
order_prefix = "config-test"
"#;

#[test]
fn dry_run_accepts_public_only_configuration() {
    let config = AppConfig::parse_and_validate(PUBLIC_ONLY, RunMode::DryRun);
    assert!(config.is_ok());
}

#[test]
fn execute_requires_complete_credentials_and_private_stream() {
    let error = AppConfig::parse_and_validate(PUBLIC_ONLY, RunMode::Execute).unwrap_err();
    assert_eq!(error.to_string(), "execute mode requires API credentials");
}

#[test]
fn partial_credentials_are_rejected_in_every_mode() {
    let raw = format!("{PUBLIC_ONLY}\napi_key = \"key-only\"\n");
    let error = AppConfig::parse_and_validate(&raw, RunMode::DryRun).unwrap_err();
    assert_eq!(error.to_string(), "api_key and secret must be configured together");
}

#[test]
fn configuration_errors_do_not_expose_secret_values() {
    let sentinel = "DO_NOT_LEAK_THIS_SECRET";
    let raw = format!(
        "{PUBLIC_ONLY}\napi_key = \"key\"\nsecret = \"{sentinel}\"\nbroken = [\n"
    );
    let error = AppConfig::parse_and_validate(&raw, RunMode::Execute).unwrap_err();
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(sentinel));
}
