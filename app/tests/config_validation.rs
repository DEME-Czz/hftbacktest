use hft_app::{config::AppConfig, ports::RunMode};

const PUBLIC_ONLY: &str = r#"
public_stream_url = "wss://example.test/ws"
api_url = "https://example.test"
order_prefix = "config-test"
"#;

#[derive(Clone, Copy)]
struct ExecutionEndpointProfile {
    name: &'static str,
    api: &'static str,
    public_stream: &'static str,
    private_stream: &'static str,
}

const EXECUTION_ENDPOINT_PROFILES: [ExecutionEndpointProfile; 3] = [
    ExecutionEndpointProfile {
        name: "production",
        api: "https://fapi.binance.com",
        public_stream: "wss://fstream.binance.com/public/stream",
        private_stream: "wss://fstream.binance.com/private/ws/{listen_key}",
    },
    ExecutionEndpointProfile {
        name: "demo",
        api: "https://demo-fapi.binance.com",
        public_stream: "wss://demo-fstream.binance.com/ws",
        private_stream: "wss://demo-fstream.binance.com/ws/{listen_key}",
    },
    ExecutionEndpointProfile {
        name: "testnet",
        api: "https://testnet.binancefuture.com",
        public_stream: "wss://stream.binancefuture.com/ws",
        private_stream: "wss://stream.binancefuture.com/ws/{listen_key}",
    },
];

fn execution_config(
    api: &str,
    public_stream: &str,
    private_stream: &str,
    allow_test_endpoints: bool,
) -> String {
    format!(
        r#"
public_stream_url = "{public_stream}"
private_stream_url = "{private_stream}"
api_url = "{api}"
order_prefix = "config-test"
api_key = "configured-key"
secret = "configured-secret"
allow_test_endpoints = {allow_test_endpoints}
"#
    )
}

fn endpoint_mutations(url: &str) -> [(&'static str, String); 5] {
    let scheme_end = url.find("://").expect("test endpoint has a scheme") + 3;
    let authority_end = url[scheme_end..]
        .find('/')
        .map_or(url.len(), |path_start| scheme_end + path_start);

    [
        (
            "userinfo",
            format!("{}operator@{}", &url[..scheme_end], &url[scheme_end..]),
        ),
        ("query", format!("{url}?unexpected=true")),
        ("fragment", format!("{url}#unexpected")),
        (
            "non-default port",
            format!("{}:444{}", &url[..authority_end], &url[authority_end..]),
        ),
        ("unexpected path", format!("{url}/unexpected")),
    ]
}

fn assert_untrusted_execution_config(raw: &str, context: &str) {
    let error = match AppConfig::parse_and_validate(raw, RunMode::Execute) {
        Ok(_) => panic!("{context} unexpectedly passed execution validation"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "execute mode requires a matched Binance endpoint environment",
        "{context}"
    );
}

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

#[test]
fn execute_rejects_credentials_sent_to_untrusted_https_hosts() {
    let raw = r#"
public_stream_url = "wss://stream.attacker.example/ws"
private_stream_url = "wss://stream.attacker.example/ws/{listen_key}"
api_url = "https://api.attacker.example"
order_prefix = "config-test"
api_key = "configured-key"
secret = "configured-secret"
"#;

    let error = AppConfig::parse_and_validate(raw, RunMode::Execute).unwrap_err();

    assert_eq!(
        error.to_string(),
        "execute mode requires a matched Binance endpoint environment"
    );
}

#[test]
fn execute_requires_explicit_opt_in_for_loopback_endpoints() {
    let base = r#"
public_stream_url = "ws://127.0.0.1:18080/ws"
private_stream_url = "ws://127.0.0.1:18081/ws/{listen_key}"
api_url = "http://127.0.0.1:18082"
order_prefix = "config-test"
api_key = "configured-key"
secret = "configured-secret"
"#;

    assert!(AppConfig::parse_and_validate(base, RunMode::Execute).is_err());
    let explicitly_allowed = format!("{base}\nallow_test_endpoints = true\n");
    assert!(AppConfig::parse_and_validate(&explicitly_allowed, RunMode::Execute).is_ok());
}

#[test]
fn execute_accepts_canonical_binance_endpoint_profiles() {
    for profile in EXECUTION_ENDPOINT_PROFILES {
        let raw = execution_config(
            profile.api,
            profile.public_stream,
            profile.private_stream,
            false,
        );
        assert!(
            AppConfig::parse_and_validate(&raw, RunMode::Execute).is_ok(),
            "{} profile should remain valid",
            profile.name
        );
    }
}

#[test]
fn execute_rejects_noncanonical_urls_for_every_binance_endpoint_profile() {
    for profile in EXECUTION_ENDPOINT_PROFILES {
        for (surface, original, endpoint_index) in [
            ("api", profile.api, 0),
            ("public stream", profile.public_stream, 1),
            ("private stream", profile.private_stream, 2),
        ] {
            for (mutation, mutated) in endpoint_mutations(original) {
                let (api, public_stream, private_stream) = match endpoint_index {
                    0 => (
                        mutated.as_str(),
                        profile.public_stream,
                        profile.private_stream,
                    ),
                    1 => (profile.api, mutated.as_str(), profile.private_stream),
                    2 => (profile.api, profile.public_stream, mutated.as_str()),
                    _ => unreachable!(),
                };
                let raw = execution_config(api, public_stream, private_stream, false);
                let context = format!("{} {surface} with {mutation}", profile.name);
                assert_untrusted_execution_config(&raw, &context);
            }
        }
    }
}

#[test]
fn execute_loopback_opt_in_only_allows_http_and_ws_transports() {
    let api = "http://127.0.0.1:18082";
    let public_stream = "ws://127.0.0.1:18080/ws";
    let private_stream = "ws://127.0.0.1:18081/ws/{listen_key}";
    let canonical = execution_config(api, public_stream, private_stream, true);
    assert!(AppConfig::parse_and_validate(&canonical, RunMode::Execute).is_ok());

    for (surface, api, public_stream, private_stream) in [
        (
            "api",
            "https://127.0.0.1:18082",
            public_stream,
            private_stream,
        ),
        (
            "public stream",
            api,
            "wss://127.0.0.1:18080/ws",
            private_stream,
        ),
        (
            "private stream",
            api,
            public_stream,
            "wss://127.0.0.1:18081/ws/{listen_key}",
        ),
    ] {
        let raw = execution_config(api, public_stream, private_stream, true);
        assert_untrusted_execution_config(&raw, &format!("secure loopback {surface}"));
    }
}
