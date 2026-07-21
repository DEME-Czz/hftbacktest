use algo::gridtrading_with_alpha;
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
#[path = "support/live_state.rs"]
mod live_state;

fn prepare_live() -> LiveBot<IceoryxUnifiedChannel, HashMapMarketDepth> {
    LiveBotBuilder::new()
        .register(Instrument::new(
            "binancefutures-prod",
            "dogeusdt",
            0.00001,
            1.0,
            HashMapMarketDepth::new(0.00001, 1.0),
            0,
        ))
        .build()
        .unwrap()
}

fn main() {
    tracing_subscriber::fmt::init();

    let mut hbt = prepare_live();
    let initial_state = live_state::wait_for_account_state(&mut hbt, 0, Duration::from_secs(30))
        .unwrap_or_else(|error| panic!("refusing to start live trading: {error}"));
    info!(
        ?initial_state,
        "Binance account state is ready; starting grid trading."
    );

    let relative_half_spread = 0.001;
    let relative_grid_interval = 0.001;
    let grid_num = 10;
    let min_grid_step = 0.00001; // DOGEUSDT tick size
    let skew = relative_half_spread / grid_num as f64;
    // At the current demo price this stays above Binance's 5 USDT minimum notional.
    let order_qty = 100.0;
    let max_position = grid_num as f64 * order_qty;

    let mut recorder = LoggingRecorder::new();
    let mut dataset_recorder = alpha_dataset::OptionalDatasetRecorder::from_env()
        .unwrap_or_else(|error| panic!("refusing to start Alpha dataset collection: {error:#}"));
    if dataset_recorder.is_enabled() {
        info!("Alpha dataset collection is enabled on the existing strategy market-data stream.");
    }
    let alpha_model = match std::env::var("HFT_ALPHA_MODEL_PATH") {
        Ok(path) => RuntimeAlphaModel::load(&path)
            .unwrap_or_else(|error| panic!("refusing to load Alpha model {path}: {error}")),
        Err(std::env::VarError::NotPresent) => RuntimeAlphaModel::flat(),
        Err(error) => panic!("invalid HFT_ALPHA_MODEL_PATH: {error}"),
    };
    let trained = alpha_model.is_trained();
    info!(trained, "Alpha inference backend is ready.");
    let alpha_config = if trained {
        AlphaConfig {
            confidence_threshold: 0.90,
            calibrated_return: 0.0002,
            max_relative_offset: 0.0002,
            smoothing: 0.25,
        }
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
        relative_half_spread,
        relative_grid_interval,
        grid_num,
        min_grid_step,
        skew,
        order_qty,
        max_position,
    )
    .unwrap();
    dataset_recorder.flush().unwrap();
    hbt.close().unwrap();
}
