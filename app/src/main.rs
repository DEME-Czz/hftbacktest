use std::{collections::{HashMap, HashSet}, fs::read_to_string, process::exit, time::Duration};

use clap::Parser;
use hft_app::{
    binancefutures::BinanceFutures,
    connector::{Connector, ConnectorBuilder, PublishEvent, RunMode},
    execution::LiveExecutor,
    risk::RiskGate,
    runtime::{LiveStrategyRuntime, RuntimeConfig},
};
use hftbacktest::{strategy::BuiltinStrategy, types::LiveEvent};
use serde::Deserialize;
use tokio::{select, signal, sync::mpsc::unbounded_channel, time};
use tracing::{error, info, trace, warn};

#[derive(Parser, Debug)]
#[command(version, about = "Binance USD-M Futures in-process strategy runtime")]
struct Args {
    /// Binance + strategy TOML configuration.
    config: String,
    /// Actually submit/cancel orders. Without this flag the runtime is decision-only dry-run.
    #[arg(long)]
    execute: bool,
}

#[derive(Deserialize)]
struct AuthConfig {
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    secret: String,
}

fn live_symbol(event: &LiveEvent) -> Option<&str> {
    match event {
        LiveEvent::Feed { symbol, .. }
        | LiveEvent::Order { symbol, .. }
        | LiveEvent::Position { symbol, .. } => Some(symbol.as_str()),
        LiveEvent::BatchStart | LiveEvent::BatchEnd | LiveEvent::Error(_) => None,
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let config = read_to_string(&args.config)
        .map_err(|error| error!(?error, path = args.config, "failed to read configuration"))
        .unwrap();

    let runtime_config: RuntimeConfig = toml::from_str(&config)
        .map_err(|error| error!(?error, "failed to parse strategy/risk configuration"))
        .unwrap();

    if runtime_config.strategies.is_empty() {
        error!("no [[strategies]] entries configured");
        exit(2);
    }

    if args.execute {
        let auth: AuthConfig = toml::from_str(&config)
            .map_err(|error| error!(?error, "failed to parse Binance credentials"))
            .unwrap();
        if auth.api_key.is_empty() || auth.secret.is_empty() {
            error!("--execute requires non-empty api_key and secret");
            exit(2);
        }
    }

    let mut runtimes: HashMap<String, LiveStrategyRuntime<BuiltinStrategy>> = HashMap::new();

    for strategy_config in &runtime_config.strategies {
        let symbol = strategy_config.symbol.to_lowercase();
        if runtimes.contains_key(&symbol) {
            error!(%symbol, "only one built-in strategy per symbol is currently supported");
            exit(2);
        }
        if strategy_config.tick_size <= 0.0 || strategy_config.lot_size <= 0.0 {
            error!(%symbol, "tick_size and lot_size must be positive");
            exit(2);
        }
        let strategy = strategy_config
            .build_strategy()
            .map_err(|reason| error!(%symbol, reason, "invalid strategy configuration"))
            .unwrap();
        runtimes.insert(
            symbol.clone(),
            LiveStrategyRuntime::new(
                symbol,
                strategy_config.tick_size,
                strategy_config.lot_size,
                strategy,
            ),
        );
    }

    let mut connector = BinanceFutures::build_from(&config)
        .map_err(|error| error!(?error, "failed to build Binance USD-M Futures adapter"))
        .unwrap();

    let (tx, mut rx) = unbounded_channel();
    connector.run(RunMode::from_execute(args.execute), tx.clone());
    for symbol in runtimes.keys() {
        connector.register(symbol.clone());
    }

    let executor = LiveExecutor::new(args.execute, RiskGate::new(runtime_config.risk));
    let mut position_ready = HashSet::new();

    info!(execute = args.execute, symbols = runtimes.len(), "Binance USD-M Futures strategy runtime started");
    if !args.execute {
        info!("dry-run mode: strategy decisions are evaluated but no orders are sent; pass --execute to trade");
    }

    loop {
        select! {
            _ = signal::ctrl_c() => {
                info!("shutdown requested");
                if args.execute {
                    let active_orders = {
                        let manager = connector.order_manager();
                        let manager = manager.lock().unwrap();
                        runtimes.keys().flat_map(|symbol| {
                            manager.orders(Some(symbol.clone()))
                                .into_iter()
                                .map(|order| (symbol.clone(), order))
                                .collect::<Vec<_>>()
                        }).collect::<Vec<_>>()
                    };
                    if !active_orders.is_empty() {
                        info!(count = active_orders.len(), "canceling active strategy orders before shutdown");
                        for (symbol, order) in active_orders {
                            connector.cancel(symbol, order, tx.clone());
                        }
                        time::sleep(Duration::from_secs(2)).await;
                    }
                }
                break;
            }
            event = rx.recv() => {
                match event {
                    Some(PublishEvent::LiveEvent(live)) => {
                        trace!(?live, "runtime event");
                        if let LiveEvent::Position { symbol, .. } = &live {
                            if runtimes.contains_key(symbol) && position_ready.insert(symbol.clone()) {
                                info!(%symbol, "initial position state synchronized");
                            }
                        }
                        if let Some(symbol) = live_symbol(&live)
                            && let Some(runtime) = runtimes.get_mut(symbol)
                        {
                            runtime.apply(&live);
                        }
                        if let LiveEvent::Error(error) = &live {
                            warn!(?error, "Binance live runtime error");
                        }
                    }
                    Some(PublishEvent::BatchEnd(_)) => {
                        for runtime in runtimes.values_mut() {
                            if !runtime.take_depth_dirty() {
                                continue;
                            }
                            if args.execute && !position_ready.contains(runtime.symbol()) {
                                trace!(symbol = runtime.symbol(), "waiting for initial position synchronization");
                                continue;
                            }
                            let commands = runtime.decide();
                            if !commands.is_empty() {
                                executor.execute(&connector, &tx, runtime, commands);
                            }
                        }
                    }
                    Some(PublishEvent::BatchStart(_)) => {}
                    Some(PublishEvent::RegisterInstrument { symbol, tick_size, lot_size, .. }) => {
                        trace!(%symbol, tick_size, lot_size, "instrument registered");
                    }
                    None => break,
                }
            }
        }
    }
}
