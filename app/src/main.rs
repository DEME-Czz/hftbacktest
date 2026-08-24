use std::fs::read_to_string;

use anyhow::{Context, Result};
use clap::Parser;
use hft_app::{
    config::AppConfig,
    exchange::binance_usdm::BinanceFutures,
    live::service::{LiveService, build_runtimes},
    ports::RunMode,
};

#[derive(Parser, Debug)]
#[command(version, about = "Binance USD-M Futures in-process strategy runtime")]
struct Args {
    /// Binance + strategy TOML configuration.
    config: String,
    /// Actually submit/cancel orders. Without this flag the runtime is decision-only dry-run.
    #[arg(long)]
    execute: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let mode = RunMode::from_execute(args.execute);
    let raw = read_to_string(&args.config)
        .with_context(|| format!("failed to read configuration: {}", args.config))?;
    let config = AppConfig::parse_and_validate(&raw, mode)?;
    let runtimes = build_runtimes(&config.runtime.strategies)?;
    let connector = BinanceFutures::new(config.exchange);

    LiveService::new(connector, runtimes, config.runtime.risk, mode)
        .run()
        .await
}
