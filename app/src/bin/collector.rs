use std::{
    fs::{File, read_to_string},
    io::{BufWriter, Write},
    process::exit,
};

use clap::Parser;
use hft_app::{
    binancefutures::BinanceFutures,
    connector::{Connector, ConnectorBuilder, PublishEvent},
};
use hftbacktest::types::LiveEvent;
use tokio::{select, signal, sync::mpsc::unbounded_channel};
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
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let config = read_to_string(&args.config)
        .map_err(|error| error!(?error, path = args.config, "failed to read configuration"))
        .unwrap();

    let mut connector = BinanceFutures::build_from(&config)
        .map_err(|error| error!(?error, "failed to build Binance adapter"))
        .unwrap();

    let file = File::create(&args.path)
        .map_err(|error| error!(?error, path = args.path, "failed to create collector output"))
        .unwrap();
    let mut writer = BufWriter::new(file);
    writeln!(writer, "symbol,ev,exch_ts,local_ts,px,qty,order_id,ival,fval").unwrap();

    let (tx, mut rx) = unbounded_channel();
    connector.run_market_data_only(tx);
    connector.register(args.symbol);

    info!(path = args.path, "normalized collector started");
    loop {
        select! {
            _ = signal::ctrl_c() => break,
            message = rx.recv() => match message {
                Some(PublishEvent::LiveEvent(LiveEvent::Feed { symbol, event })) => {
                    if writeln!(
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
                    ).is_err() {
                        error!("failed to write normalized event");
                        exit(1);
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
    }
    writer.flush().unwrap();
}
