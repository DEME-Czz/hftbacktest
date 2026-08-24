use std::{
    fs::{File, read_to_string},
    io::{BufWriter, Write},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use hft_app::{
    config::AppConfig,
    exchange::binance_usdm::BinanceFutures,
    ports::{MarketDataSource, PublishEvent, RunMode},
};
use hftbacktest::types::LiveEvent;
use tokio::{select, signal, sync::mpsc::unbounded_channel, time};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(version, about = "Collect normalized Binance USD-M Futures events")]
struct Args {
    /// Binance connector TOML configuration. API credentials are ignored by collector.
    config: String,
    /// Output CSV file.
    path: String,
    /// One Binance USD-M Futures symbol.
    symbol: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let raw = read_to_string(&args.config)
        .with_context(|| format!("failed to read configuration: {}", args.config))?;
    let config = AppConfig::parse_and_validate(&raw, RunMode::DryRun)?;
    let mut connector = BinanceFutures::new(config.exchange)?;

    let file = File::create(&args.path)
        .with_context(|| format!("failed to create collector output: {}", args.path))?;
    let mut writer = BufWriter::new(file);
    writeln!(
        writer,
        "symbol,ev,exch_ts,local_ts,px,qty,order_id,ival,fval"
    )?;
    writer.flush()?;

    let (tx, mut rx) = unbounded_channel();
    connector.register(args.symbol);
    connector.start_market_data(tx);

    let mut flush_interval = time::interval(Duration::from_secs(1));
    let mut event_count: u64 = 0;

    info!(path = args.path, "normalized collector started");
    loop {
        select! {
            _ = signal::ctrl_c() => break,
            _ = flush_interval.tick() => writer.flush()?,
            message = rx.recv() => match message {
                Some(PublishEvent::LiveEvent(LiveEvent::Feed { symbol, event })) => {
                    if let Err(error) = writeln!(
                        writer,
                        "{symbol},{},{},{},{},{},{},{},{}",
                        event.ev,
                        event.exch_ts,
                        event.local_ts,
                        event.px,
                        event.qty,
                        event.order_id,
                        event.ival,
                        event.fval,
                    ) {
                        error!(?error, "failed to write normalized event");
                        return Err(error.into());
                    }
                    event_count += 1;
                    if event_count == 1 {
                        info!(%symbol, "first normalized market event received");
                    } else if event_count.is_multiple_of(10_000) {
                        info!(event_count, "normalized events collected");
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
    }
    writer.flush()?;
    info!(event_count, "normalized collector stopped");
    Ok(())
}
