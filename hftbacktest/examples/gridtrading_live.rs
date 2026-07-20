use algo::gridtrading;
use std::time::Duration;

use hftbacktest::{
    live::{
        Instrument, LiveBot, LiveBotBuilder, LoggingRecorder, ipc::iceoryx::IceoryxUnifiedChannel,
    },
    prelude::{Bot, HashMapMarketDepth},
};
use tracing::info;

mod algo;
#[path = "support/live_state.rs"]
mod live_state;

fn prepare_live() -> LiveBot<IceoryxUnifiedChannel, HashMapMarketDepth> {
    LiveBotBuilder::new()
        .register(Instrument::new(
            "binancefutures-prod",
            "synusdt",
            0.0001,
            1.0,
            HashMapMarketDepth::new(0.0001, 1.0),
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

    let relative_half_spread = 0.005;
    let relative_grid_interval = 0.005;
    let grid_num = 10;
    let min_grid_step = 0.0001; // DOGEUSDT tick size
    let skew = relative_half_spread / grid_num as f64;
    // At the current demo price this stays above Binance's 5 USDT minimum notional.
    let order_qty = 100.0;
    let max_position = grid_num as f64 * order_qty;

    let mut recorder = LoggingRecorder::new();
    gridtrading(
        &mut hbt,
        &mut recorder,
        relative_half_spread,
        relative_grid_interval,
        grid_num,
        min_grid_step,
        skew,
        order_qty,
        max_position,
    )
    .unwrap();
    hbt.close().unwrap();
}
