use hft_app::{config::AppConfig, ports::RunMode};

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
fn execute_rejects_credentials_without_a_private_stream_template() {
    let raw =
        format!("{PUBLIC_ONLY}\napi_key = \"configured-key\"\nsecret = \"configured-secret\"\n");
    let error = AppConfig::parse_and_validate(&raw, RunMode::Execute).unwrap_err();
    assert_eq!(
        error.to_string(),
        "execute mode requires a secure private_stream_url with {listen_key}"
    );
}

#[test]
fn remote_endpoints_must_use_encrypted_transports() {
    let raw = PUBLIC_ONLY
        .replace("wss://example.test/ws", "ws://example.test/ws")
        .replace("https://example.test", "http://example.test");
    let error = AppConfig::parse_and_validate(&raw, RunMode::DryRun).unwrap_err();
    assert_eq!(
        error.to_string(),
        "remote Binance endpoints must use wss/https"
    );
}

#[test]
fn partial_credentials_are_rejected_in_every_mode() {
    let raw = format!("{PUBLIC_ONLY}\napi_key = \"key-only\"\n");
    let error = AppConfig::parse_and_validate(&raw, RunMode::DryRun).unwrap_err();
    assert_eq!(
        error.to_string(),
        "api_key and secret must be configured together"
    );
}

#[test]
fn configuration_errors_do_not_expose_secret_values() {
    let sentinel = "DO_NOT_LEAK_THIS_SECRET";
    let raw = format!("{PUBLIC_ONLY}\napi_key = \"key\"\nsecret = \"{sentinel}\"\nbroken = [\n");
    let error = AppConfig::parse_and_validate(&raw, RunMode::Execute).unwrap_err();
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(sentinel));
}

#[test]
fn parsed_configuration_debug_output_redacts_credentials() {
    let key = "DO_NOT_LEAK_THIS_KEY";
    let secret = "DO_NOT_LEAK_THIS_SECRET";
    let raw = format!("{PUBLIC_ONLY}\napi_key = \"{key}\"\nsecret = \"{secret}\"\n");
    let config = AppConfig::parse_and_validate(&raw, RunMode::DryRun).unwrap();
    let rendered = format!("{config:?}");

    assert!(!rendered.contains(key));
    assert!(!rendered.contains(secret));
    assert!(rendered.contains("credentials_configured"));
}

#[test]
fn non_finite_risk_limits_are_rejected_at_startup() {
    let raw = format!("{PUBLIC_ONLY}\n[risk]\nmax_order_qty = nan\n");
    let error = AppConfig::parse_and_validate(&raw, RunMode::DryRun).unwrap_err();
    assert_eq!(error.to_string(), "risk limits must be finite and positive");
}
