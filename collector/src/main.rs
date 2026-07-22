use clap::Parser;
use tokio::{self, select, signal, sync::mpsc::unbounded_channel};
use tracing::{error, info};

use crate::{
    config::{CollectorConfig, Exchange},
    file::Writer,
};

mod binance;
mod binancefuturescm;
mod binancefuturesum;
mod bybit;
mod config;
mod error;
mod file;
mod hyperliquid;
mod proxy;
mod throttler;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the TOML file containing every collector parameter.
    config: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), anyhow::Error> {
    let args = Args::parse();
    let config = CollectorConfig::load(&args.config)?;
    std::fs::create_dir_all(&config.output_path)?;

    tracing_subscriber::fmt::init();

    let (writer_tx, mut writer_rx) = unbounded_channel();

    let _handle = match config.exchange {
        Exchange::BinanceFuturesUm => tokio::spawn(binancefuturesum::run_collection(
            config.streams,
            config.symbols,
            config.proxy,
            writer_tx,
        )),
        Exchange::BinanceFuturesCm => tokio::spawn(binancefuturescm::run_collection(
            config.streams,
            config.symbols,
            writer_tx,
        )),
        Exchange::BinanceSpot => tokio::spawn(binance::run_collection(
            config.streams,
            config.symbols,
            writer_tx,
        )),
        Exchange::Bybit => tokio::spawn(bybit::run_collection(
            config.streams,
            config.symbols,
            writer_tx,
        )),
        Exchange::Hyperliquid => tokio::spawn(hyperliquid::run_collection(
            config.streams,
            config.symbols,
            writer_tx,
        )),
    };

    let mut writer = Writer::new(&config.output_path);
    loop {
        select! {
            _ = signal::ctrl_c() => {
                info!("ctrl-c received");
                break;
            }
            r = writer_rx.recv() => match r {
                Some((recv_time, symbol, data)) => {
                    if let Err(error) = writer.write(recv_time, symbol, data) {
                        error!(?error, "write error");
                        break;
                    }
                }
                None => {
                    break;
                }
            }
        }
    }
    // let _ = handle.await;
    Ok(())
}
