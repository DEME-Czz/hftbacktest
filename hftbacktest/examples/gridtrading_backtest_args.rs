use algo::gridtrading;
use clap::Parser;
use hftbacktest::{
    backtest::{
        Backtest, DataSource, ExchangeKind, L2AssetBuilder,
        assettype::LinearAsset,
        data::read_npz_file,
        models::{
            CommonFees, IntpOrderLatency, PowerProbQueueFunc3, ProbQueueModel, TradingValueFeeModel,
        },
        recorder::BacktestRecorder,
    },
    prelude::{ApplySnapshot, Bot, HashMapMarketDepth},
};
use serde::Deserialize;

mod algo;

#[derive(Parser, Debug)]
#[command(about = None, long_about = None)]
struct Cli {
    /// TOML containing every backtest parameter.
    config: String,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct Args {
    name: String,
    output_path: String,
    data_files: Vec<String>,
    initial_snapshot: Option<String>,
    latency_files: Vec<String>,
    tick_size: f64,
    lot_size: f64,
    relative_half_spread: f64,
    relative_grid_interval: f64,
    skew: f64,
    grid_num: usize,
    min_grid_step: Option<f64>,
    order_qty: f64,
    max_position: f64,
    maker_fee: f64,
    taker_fee: f64,
}

fn prepare_backtest(
    latency_files: Vec<String>,
    data_files: Vec<String>,
    initial_snapshot: Option<String>,
    tick_size: f64,
    lot_size: f64,
    maker_fee: f64,
    taker_fee: f64,
) -> Backtest<HashMapMarketDepth> {
    let latency_model = IntpOrderLatency::new(
        latency_files
            .iter()
            .map(|file| DataSource::File(file.clone()))
            .collect(),
        0,
    );
    let asset_type = LinearAsset::new(1.0);
    let queue_model = ProbQueueModel::new(PowerProbQueueFunc3::new(3.0));

    let hbt = Backtest::builder()
        .add_asset(
            L2AssetBuilder::new()
                .data(
                    data_files
                        .iter()
                        .map(|file| DataSource::File(file.clone()))
                        .collect(),
                )
                .latency_model(latency_model)
                .asset_type(asset_type)
                .fee_model(TradingValueFeeModel::new(CommonFees::new(
                    maker_fee, taker_fee,
                )))
                .exchange(ExchangeKind::NoPartialFillExchange)
                .queue_model(queue_model)
                .depth(move || {
                    let mut depth = HashMapMarketDepth::new(tick_size, lot_size);
                    if let Some(file) = initial_snapshot.as_ref() {
                        depth.apply_snapshot(&read_npz_file(file, "data").unwrap());
                    }
                    depth
                })
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    hbt
}

fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.config)
        .unwrap_or_else(|error| panic!("read {}: {error}", cli.config));
    let args: Args =
        toml::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", cli.config));

    let mut hbt = prepare_backtest(
        args.latency_files,
        args.data_files,
        args.initial_snapshot,
        args.tick_size,
        args.lot_size,
        args.maker_fee,
        args.taker_fee,
    );
    let mut recorder = BacktestRecorder::new(&hbt);
    gridtrading(
        &mut hbt,
        &mut recorder,
        args.relative_half_spread,
        args.relative_grid_interval,
        args.grid_num,
        args.min_grid_step.unwrap_or(args.tick_size as f64),
        args.skew,
        args.order_qty,
        args.max_position,
    )
    .unwrap();
    hbt.close().unwrap();
    recorder.to_csv(args.name, args.output_path).unwrap();
}
