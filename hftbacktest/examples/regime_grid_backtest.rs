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
use regime_grid::{GridConfig, regime_gridtrading};

mod regime_grid;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    name: String,
    #[arg(long)]
    output_path: String,
    #[arg(long, num_args = 1..)]
    data_files: Vec<String>,
    #[arg(long)]
    initial_snapshot: Option<String>,
    #[arg(long, num_args = 1..)]
    latency_files: Vec<String>,
    #[arg(long)]
    tick_size: f64,
    #[arg(long)]
    lot_size: f64,
    #[arg(long, default_value_t = 0.0005)]
    relative_half_spread: f64,
    #[arg(long, default_value_t = 0.0005)]
    relative_grid_interval: f64,
    #[arg(long, default_value_t = 10)]
    sideways_levels: usize,
    #[arg(long, default_value_t = 6)]
    trend_levels: usize,
    #[arg(long, default_value_t = 4)]
    reduce_levels: usize,
    #[arg(long)]
    min_grid_step: Option<f64>,
    #[arg(long, default_value_t = 1.0)]
    order_qty: f64,
    #[arg(long, default_value_t = 10.0)]
    max_long: f64,
    #[arg(long, default_value_t = 10.0)]
    max_short: f64,
    /// Causal return window used by the example classifier.
    #[arg(long, default_value_t = 60_000)]
    return_horizon_ms: i64,
    /// Absolute log-return that represents one unit of directional evidence.
    #[arg(long, default_value_t = 0.002)]
    trend_return_threshold: f64,
    /// Validated multinomial Group-LASSO JSON. Omit to run the causal return-rule baseline.
    #[arg(long)]
    model: Option<String>,
    #[arg(long, default_value_t = 60_000)]
    prediction_horizon_ms: i64,
    #[arg(long, default_value_t = 20.0)]
    max_spread_bps: f64,
    #[arg(long, default_value_t = 10.0)]
    max_position_hard: f64,
    #[arg(long, default_value_t = 2_000)]
    market_data_stale_ms: i64,
    #[arg(long, default_value_t = 100.0)]
    max_daily_loss: f64,
    #[arg(long, default_value_t = 150.0)]
    max_drawdown: f64,
    #[arg(long, default_value_t = 0.5)]
    edge_full: f64,
    #[arg(long, default_value_t = 0.5)]
    min_directional_limit_ratio: f64,
    #[arg(long, default_value_t = 0.5)]
    alpha_multiplier: f64,
    #[arg(long, default_value_t = 1.0)]
    alpha_max_grid_intervals: f64,
    #[arg(long)]
    inventory_skew: Option<f64>,
    #[arg(long, default_value_t = 0.7)]
    reduce_spread_factor: f64,
    #[arg(long, default_value_t = 4.0)]
    volatility_shock_multiple: f64,
    #[arg(long, default_value_t = -0.00005)]
    maker_fee: f64,
    #[arg(long, default_value_t = 0.0007)]
    taker_fee: f64,
}

fn prepare_backtest(args: &Args) -> Backtest<HashMapMarketDepth> {
    let latency_model = IntpOrderLatency::new(
        args.latency_files
            .iter()
            .map(|file| DataSource::File(file.clone()))
            .collect(),
        0,
    );
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
                .latency_model(latency_model)
                .asset_type(LinearAsset::new(1.0))
                .fee_model(TradingValueFeeModel::new(CommonFees::new(
                    args.maker_fee,
                    args.taker_fee,
                )))
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

fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let config = GridConfig {
        relative_half_spread: args.relative_half_spread,
        relative_grid_interval: args.relative_grid_interval,
        min_grid_step: args.min_grid_step.unwrap_or(args.tick_size),
        sideways_levels: args.sideways_levels,
        trend_levels: args.trend_levels,
        reduce_levels: args.reduce_levels,
        order_qty: args.order_qty,
        max_long: args.max_long,
        max_short: args.max_short,
        return_horizon_ns: args.return_horizon_ms * 1_000_000,
        trend_return_threshold: args.trend_return_threshold,
        model_path: args.model.clone(),
        prediction_horizon_ms: args.prediction_horizon_ms,
        max_spread_bps: args.max_spread_bps,
        max_position_hard: args.max_position_hard,
        market_data_stale_ns: args.market_data_stale_ms * 1_000_000,
        max_daily_loss: args.max_daily_loss,
        max_drawdown: args.max_drawdown,
        edge_full: args.edge_full,
        min_directional_limit_ratio: args.min_directional_limit_ratio,
        alpha_multiplier: args.alpha_multiplier,
        alpha_max_grid_intervals: args.alpha_max_grid_intervals,
        inventory_skew: args
            .inventory_skew
            .unwrap_or(args.relative_half_spread / args.sideways_levels.max(1) as f64),
        reduce_spread_factor: args.reduce_spread_factor,
        volatility_shock_multiple: args.volatility_shock_multiple,
    };
    let mut hbt = prepare_backtest(&args);
    let mut recorder = BacktestRecorder::new(&hbt);
    regime_gridtrading(&mut hbt, &mut recorder, config).unwrap();
    hbt.close().unwrap();
    recorder.to_csv(args.name, args.output_path).unwrap();
}
