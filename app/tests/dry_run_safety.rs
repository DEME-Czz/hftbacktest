use std::{
    fs,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{net::TcpListener, process::Command, time::timeout};

#[tokio::test]
async fn dry_run_with_credentials_does_not_touch_private_api() {
    let private_api = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private_api_addr = private_api.local_addr().unwrap();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_path = std::env::temp_dir().join(format!(
        "hftbacktest-dry-run-{}-{unique}.toml",
        std::process::id()
    ));

    let config = format!(
        r#"
public_stream_url = "ws://127.0.0.1:9"
private_stream_url = "ws://127.0.0.1:9/ws/{{listen_key}}"
api_url = "http://{private_api_addr}"
order_prefix = "safety-test"
api_key = "configured-key"
secret = "configured-secret"

[risk]
max_order_qty = 0.001
max_order_notional = 100.0
max_position = 0.003
max_open_orders = 4

[[strategies]]
symbol = "BTCUSDT"
kind = "grid"
tick_size = 0.1
lot_size = 0.001
relative_half_spread = 0.0005
relative_grid_interval = 0.0005
grid_num = 2
min_grid_step = 0.1
skew = 0.00025
order_qty = 0.001
max_position = 0.003
"#
    );
    fs::write(&config_path, config).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_hft-app"))
        .arg(&config_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let private_request = timeout(Duration::from_secs(3), private_api.accept()).await;

    child.kill().await.unwrap();
    let _ = child.wait().await;
    fs::remove_file(config_path).unwrap();

    assert!(
        private_request.is_err(),
        "dry-run contacted the authenticated Binance API"
    );
}
