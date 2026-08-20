use std::time::Duration;

use clap::Parser;
use hftbacktest::{
    live::{
        Instrument, LiveBot, LiveBotBuilder, LoggingRecorder, ipc::iceoryx::IceoryxUnifiedChannel,
    },
    prelude::{Bot, HashMapMarketDepth},
};
use regime_grid::{GridConfig, regime_gridtrading};
use tracing::info;

#[path = "support/live_state.rs"]
mod live_state;
mod regime_grid;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value = "binancefutures-prod")]
    connector: String,
    #[arg(long)]
    symbol: String,
    #[arg(long)]
    tick_size: f64,
    #[arg(long)]
    lot_size: f64,
    /// A validated multinomial Group-LASSO JSON is mandatory in live trading.
    #[arg(long)]
    model: String,
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
    #[arg(long, default_value_t = 1.0)]
    order_qty: f64,
    #[arg(long, default_value_t = 10.0)]
    max_long: f64,
    #[arg(long, default_value_t = 10.0)]
    max_short: f64,
    #[arg(long, default_value_t = 10.0)]
    max_position_hard: f64,
    #[arg(long, default_value_t = 20.0)]
    max_spread_bps: f64,
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
    #[arg(long, default_value_t = 60_000)]
    prediction_horizon_ms: i64,
    #[arg(long, default_value_t = 2_000)]
    market_data_stale_ms: i64,
    #[arg(long, default_value_t = 30)]
    account_timeout_seconds: u64,
}

fn prepare_live(args: &Args) -> LiveBot<IceoryxUnifiedChannel, HashMapMarketDepth> {
    LiveBotBuilder::new()
        .register(Instrument::new(
            &args.connector,
            &args.symbol,
            args.tick_size,
            args.lot_size,
            HashMapMarketDepth::new(args.tick_size, args.lot_size),
            4096,
        ))
        .build()
        .unwrap()
}

fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let mut hbt = prepare_live(&args);
    let initial_state = live_state::wait_for_account_state(
        &mut hbt,
        0,
        Duration::from_secs(args.account_timeout_seconds),
    )
    .unwrap_or_else(|error| panic!("refusing to start regime grid trading: {error}"));
    info!(
        ?initial_state,
        "account state recovered; starting regime grid"
    );

    let config = GridConfig {
        relative_half_spread: args.relative_half_spread,
        relative_grid_interval: args.relative_grid_interval,
        min_grid_step: args.tick_size,
        sideways_levels: args.sideways_levels,
        trend_levels: args.trend_levels,
        reduce_levels: args.reduce_levels,
        order_qty: args.order_qty,
        max_long: args.max_long,
        max_short: args.max_short,
        // Unused when a model is present; retained for the backtest rule baseline.
        return_horizon_ns: args.prediction_horizon_ms * 1_000_000,
        trend_return_threshold: 0.002,
        model_path: Some(args.model),
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
    let mut recorder = LoggingRecorder::new();
    regime_gridtrading(&mut hbt, &mut recorder, config).unwrap();
    hbt.close().unwrap();
}
