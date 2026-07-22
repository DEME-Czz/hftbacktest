use algo::gridtrading_with_alpha;
use clap::Parser;
use std::time::Duration;

use hftbacktest::{
    alpha::{AlphaConfig, AlphaEngine, RuntimeAlphaModel},
    live::{
        Instrument, LiveBot, LiveBotBuilder, LoggingRecorder, ipc::iceoryx::IceoryxUnifiedChannel,
    },
    prelude::{Bot, HashMapMarketDepth},
};
use tracing::info;

mod algo;
#[path = "support/alpha_dataset.rs"]
mod alpha_dataset;
#[path = "support/grid_live_config.rs"]
mod grid_live_config;
#[path = "support/live_state.rs"]
mod live_state;

#[derive(Parser)]
struct Args {
    /// TOML containing every live strategy parameter.
    config: String,
}

fn prepare_live(
    config: &grid_live_config::GridLiveConfig,
) -> LiveBot<IceoryxUnifiedChannel, HashMapMarketDepth> {
    LiveBotBuilder::new()
        .register(Instrument::new(
            &config.connector_name,
            &config.symbol,
            config.tick_size,
            config.lot_size,
            HashMapMarketDepth::new(config.tick_size, config.lot_size),
            0,
        ))
        .build()
        .unwrap()
}

fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let config = grid_live_config::GridLiveConfig::load(&args.config)
        .unwrap_or_else(|error| panic!("refusing to load live strategy configuration: {error:#}"));

    let mut hbt = prepare_live(&config);
    let initial_state = live_state::wait_for_account_state(
        &mut hbt,
        0,
        Duration::from_secs(config.startup_timeout_seconds),
    )
    .unwrap_or_else(|error| panic!("refusing to start live trading: {error}"));
    info!(
        ?initial_state,
        "Binance account state is ready; starting grid trading."
    );

    let mut recorder = LoggingRecorder::new();
    let mut dataset_recorder =
        alpha_dataset::OptionalDatasetRecorder::from_path(config.dataset_path.as_deref())
            .unwrap_or_else(|error| {
                panic!("refusing to start Alpha dataset collection: {error:#}")
            });
    if dataset_recorder.is_enabled() {
        info!("Alpha dataset collection is enabled on the existing strategy market-data stream.");
    }
    let alpha_model = match config.model_path.as_ref() {
        Some(path) => RuntimeAlphaModel::load(path).unwrap_or_else(|error| {
            panic!("refusing to load Alpha model {}: {error}", path.display())
        }),
        None => RuntimeAlphaModel::flat(),
    };
    let trained = alpha_model.is_trained();
    info!(trained, "Alpha inference backend is ready.");
    let alpha_config = if trained {
        config.alpha_config()
    } else {
        AlphaConfig::default()
    };
    let mut alpha_engine = AlphaEngine::new(alpha_model, alpha_config).unwrap();
    gridtrading_with_alpha(
        &mut hbt,
        &mut recorder,
        |depth| {
            if let Err(error) = dataset_recorder.record(depth) {
                tracing::error!(?error, "Alpha dataset collection failed and was disabled.");
                dataset_recorder.disable();
            }
            match alpha_engine.update(depth) {
                Ok(signal) => signal.price_offset,
                Err(error) => {
                    tracing::warn!(?error, "Alpha update failed; using a zero price offset.");
                    alpha_engine.reset();
                    0.0
                }
            }
        },
        config.relative_half_spread,
        config.relative_grid_interval,
        config.grid_num,
        config.min_grid_step,
        config.skew,
        config.order_qty,
        config.max_position,
    )
    .unwrap();
    dataset_recorder.flush().unwrap();
    hbt.close().unwrap();
}
