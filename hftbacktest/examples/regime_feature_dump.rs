use std::{
    fs::File,
    io::{BufWriter, Write},
};

use clap::Parser;
use hftbacktest::{
    backtest::{
        Backtest, DataSource, ExchangeKind, L2AssetBuilder,
        assettype::LinearAsset,
        data::read_npz_file,
        models::{
            CommonFees, ConstantLatency, PowerProbQueueFunc3, ProbQueueModel, TradingValueFeeModel,
        },
    },
    prelude::{ApplySnapshot, Bot, ElapseResult, HashMapMarketDepth, MarketDepth},
};
use regime_grid::{FEATURE_NAMES, FeatureEngine};

mod regime_grid;

/// Export the exact causal Rust runtime features used by the regime strategy.
#[derive(Parser, Debug)]
struct Args {
    #[arg(long, num_args = 1..)]
    data_files: Vec<String>,
    #[arg(long)]
    initial_snapshot: Option<String>,
    #[arg(long)]
    output: String,
    #[arg(long)]
    tick_size: f64,
    #[arg(long)]
    lot_size: f64,
    #[arg(long, default_value_t = 0.0005)]
    relative_grid_interval: f64,
    #[arg(long, default_value_t = 1_000)]
    sample_interval_ms: i64,
}

fn prepare_backtest(args: &Args) -> Backtest<HashMapMarketDepth> {
    let initial_snapshot = args.initial_snapshot.clone();
    Backtest::builder()
        .add_asset(
            L2AssetBuilder::new()
                .data(
                    args.data_files
                        .iter()
                        .map(|file| DataSource::File(file.clone()))
                        .collect(),
                )
                .latency_model(ConstantLatency::new(0, 0))
                .asset_type(LinearAsset::new(1.0))
                .fee_model(TradingValueFeeModel::new(CommonFees::new(0.0, 0.0)))
                .exchange(ExchangeKind::NoPartialFillExchange)
                .queue_model(ProbQueueModel::new(PowerProbQueueFunc3::new(3.0)))
                .last_trades_capacity(4096)
                .depth({
                    let tick_size = args.tick_size;
                    let lot_size = args.lot_size;
                    move || {
                        let mut depth = HashMapMarketDepth::new(tick_size, lot_size);
                        if let Some(file) = initial_snapshot.as_ref() {
                            depth.apply_snapshot(&read_npz_file(file, "data").unwrap());
                        }
                        depth
                    }
                })
                .build()
                .unwrap(),
        )
        .build()
        .unwrap()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.sample_interval_ms <= 0 || args.relative_grid_interval <= 0.0 {
        return Err("sample interval and relative grid interval must be positive".into());
    }
    let mut hbt = prepare_backtest(&args);
    let mut engine = FeatureEngine::new(args.sample_interval_ms * 1_000_000);
    let mut output = BufWriter::new(File::create(&args.output)?);
    writeln!(
        output,
        "timestamp_ms,mid,relative_grid_interval,{}",
        FEATURE_NAMES.join(",")
    )?;

    loop {
        match hbt.elapse(args.sample_interval_ms * 1_000_000)? {
            ElapseResult::EndOfData => break,
            ElapseResult::Ok | ElapseResult::MarketFeed | ElapseResult::OrderResponse => {}
        }
        let now = hbt.current_timestamp();
        engine.on_trades(hbt.last_trades(0), now);
        hbt.clear_last_trades(Some(0));
        if !engine.sample(hbt.depth(0), now) {
            continue;
        }
        let Some(features) = engine.snapshot() else {
            continue;
        };
        let mid = (hbt.depth(0).best_bid() + hbt.depth(0).best_ask()) / 2.0;
        write!(
            output,
            "{},{mid:.12},{:.12}",
            now / 1_000_000,
            args.relative_grid_interval
        )?;
        for value in features {
            write!(output, ",{value:.17}")?;
        }
        writeln!(output)?;
    }
    hbt.close()?;
    output.flush()?;
    Ok(())
}
