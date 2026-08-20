use std::{fs::read_to_string, process::exit};

use clap::Parser;
use hft_app::{
    binancefutures::BinanceFutures,
    connector::{Connector, ConnectorBuilder, PublishEvent},
};
use tokio::{select, signal, sync::mpsc::unbounded_channel};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(version, about = "Binance USD-M Futures in-process runtime")]
struct Args {
    /// Binance connector TOML configuration.
    config: String,
    /// Symbols to subscribe to. Binance symbols are normalized to lowercase.
    symbols: Vec<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    if args.symbols.is_empty() {
        error!("at least one symbol is required");
        exit(2);
    }

    let config = read_to_string(&args.config)
        .map_err(|error| {
            error!(?error, path = args.config, "failed to read configuration");
        })
        .unwrap();

    let mut connector = BinanceFutures::build_from(&config)
        .map_err(|error| {
            error!(?error, "failed to build Binance USD-M Futures adapter");
        })
        .unwrap();

    let (tx, mut rx) = unbounded_channel();
    connector.run(tx);
    for symbol in args.symbols {
        connector.register(symbol);
    }

    info!("Binance USD-M Futures runtime started");
    loop {
        select! {
            _ = signal::ctrl_c() => {
                info!("shutdown requested");
                break;
            }
            event = rx.recv() => {
                match event {
                    Some(PublishEvent::LiveEvent(event)) => tracing::debug!(?event, "runtime event"),
                    Some(PublishEvent::BatchStart(_)) | Some(PublishEvent::BatchEnd(_)) => {}
                    Some(PublishEvent::RegisterInstrument { .. }) => {}
                    None => break,
                }
            }
        }
    }
}
