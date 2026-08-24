use std::{
    collections::{HashMap, HashSet},
    future::Future,
    time::Duration,
};

use anyhow::{Result, bail};
use hftbacktest::{strategy::BuiltinStrategy, types::LiveEvent};
use tokio::{select, signal, sync::mpsc::unbounded_channel, time};
use tracing::{error, info, trace, warn};

use super::{
    config::LiveStrategyConfig,
    execution::LiveExecutor,
    risk::{RiskConfig, RiskGate},
    runtime::LiveStrategyRuntime,
};
use crate::ports::{LiveConnector, PublishEvent, RunMode, TradingInstrument};

pub type StrategyRuntimes = HashMap<String, LiveStrategyRuntime<BuiltinStrategy>>;

#[derive(Default)]
struct AccountReadiness {
    symbols: HashSet<String>,
    halted: HashSet<String>,
}

impl AccountReadiness {
    fn reconcile(&mut self, symbol: &str) -> bool {
        if self.halted.contains(symbol) {
            return false;
        }
        self.symbols.insert(symbol.to_string())
    }

    fn disconnect(&mut self) {
        self.symbols.clear();
    }

    fn contains(&self, symbol: &str) -> bool {
        self.symbols.contains(symbol)
    }

    fn halt(&mut self, symbol: &str) {
        self.symbols.remove(symbol);
        self.halted.insert(symbol.to_string());
    }
}

pub fn build_runtimes(configs: &[LiveStrategyConfig]) -> Result<StrategyRuntimes> {
    if configs.is_empty() {
        bail!("no [[strategies]] entries configured");
    }

    let mut runtimes = HashMap::new();
    for config in configs {
        let symbol = config.symbol.to_lowercase();
        if runtimes.contains_key(&symbol) {
            bail!("only one built-in strategy per symbol is supported: {symbol}");
        }
        if !config.tick_size.is_finite()
            || config.tick_size <= 0.0
            || !config.lot_size.is_finite()
            || config.lot_size <= 0.0
        {
            bail!("tick_size and lot_size must be positive for {symbol}");
        }
        let strategy = config.build_strategy().map_err(|reason| {
            anyhow::anyhow!("invalid strategy configuration for {symbol}: {reason}")
        })?;
        runtimes.insert(
            symbol.clone(),
            LiveStrategyRuntime::new(symbol, config.tick_size, config.lot_size, strategy),
        );
    }
    Ok(runtimes)
}

pub struct LiveService<C> {
    connector: C,
    runtimes: StrategyRuntimes,
    executor: LiveExecutor,
    mode: RunMode,
}

impl<C: LiveConnector> LiveService<C> {
    pub fn new(connector: C, runtimes: StrategyRuntimes, risk: RiskConfig, mode: RunMode) -> Self {
        Self {
            connector,
            runtimes,
            executor: LiveExecutor::new(mode.allows_trading(), RiskGate::new(risk)),
            mode,
        }
    }

    pub async fn run(self) -> Result<()> {
        self.run_until(async {
            let _ = signal::ctrl_c().await;
        })
        .await
    }

    async fn run_until<F>(mut self, shutdown: F) -> Result<()>
    where
        F: Future<Output = ()>,
    {
        let (tx, mut rx) = unbounded_channel();
        for symbol in self.runtimes.keys() {
            self.connector.register(symbol.clone());
        }
        self.connector.start_market_data(tx.clone());
        if self.mode.allows_trading() {
            let instruments = self
                .runtimes
                .values()
                .map(|runtime| TradingInstrument {
                    symbol: runtime.symbol().to_string(),
                    tick_size: runtime.tick_size(),
                })
                .collect();
            self.connector.start_account_stream(instruments, tx.clone());
        }

        let mut account_readiness = AccountReadiness::default();
        info!(
            execute = self.mode.allows_trading(),
            symbols = self.runtimes.len(),
            "Binance USD-M Futures strategy runtime started"
        );
        if !self.mode.allows_trading() {
            info!("dry-run mode: strategy decisions are evaluated but no orders are sent");
        }

        tokio::pin!(shutdown);
        loop {
            select! {
                _ = &mut shutdown => {
                    info!("shutdown requested");
                    self.cancel_before_shutdown(&tx).await;
                    break;
                }
                event = rx.recv() => match event {
                    Some(PublishEvent::LiveEvent(live)) => {
                        trace!(?live, "runtime event");
                        if let Some(symbol) = live_symbol(&live)
                            && let Some(runtime) = self.runtimes.get_mut(symbol)
                        {
                            runtime.apply(&live);
                        }
                        if let LiveEvent::Error(error) = &live {
                            warn!(?error, "Binance live runtime error");
                        }
                    }
                    Some(PublishEvent::BatchEnd) => {
                        for runtime in self.runtimes.values_mut() {
                            if !runtime.take_depth_dirty() {
                                continue;
                            }
                            if self.mode.allows_trading()
                                && !account_readiness.contains(runtime.symbol())
                            {
                                trace!(symbol = runtime.symbol(), "waiting for initial position synchronization");
                                continue;
                            }
                            let commands = runtime.decide();
                            if !commands.is_empty() {
                                self.executor.execute(&self.connector, &tx, runtime, commands);
                            }
                        }
                    }
                    Some(PublishEvent::BatchStart) => {}
                    Some(PublishEvent::AccountStreamDisconnected) => {
                        account_readiness.disconnect();
                        warn!("Binance account stream disconnected; execution paused until positions are reconciled");
                    }
                    Some(PublishEvent::AccountSnapshotReady { symbol }) => {
                        if self.runtimes.contains_key(&symbol)
                            && account_readiness.reconcile(&symbol)
                        {
                            info!(%symbol, "position and open-order state synchronized");
                        }
                    }
                    Some(PublishEvent::ExecutionUncertain { symbol }) => {
                        account_readiness.halt(&symbol);
                        error!(%symbol, "execution halted: order submission outcome is unresolved");
                    }
                    None => break,
                }
            }
        }
        Ok(())
    }

    async fn cancel_before_shutdown(&self, tx: &tokio::sync::mpsc::UnboundedSender<PublishEvent>) {
        if !self.mode.allows_trading() {
            return;
        }

        let active_orders = self
            .runtimes
            .keys()
            .flat_map(|symbol| {
                self.connector
                    .open_orders(symbol)
                    .into_iter()
                    .map(|order| (symbol.clone(), order))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if active_orders.is_empty() {
            return;
        }

        info!(
            count = active_orders.len(),
            "canceling active strategy orders before shutdown"
        );
        for (symbol, order) in active_orders {
            self.connector.cancel(symbol, order, tx.clone());
        }

        let deadline = time::Instant::now() + Duration::from_secs(2);
        loop {
            let remaining = self
                .runtimes
                .keys()
                .map(|symbol| self.connector.open_orders(symbol).len())
                .sum::<usize>();
            if remaining == 0 {
                return;
            }
            if time::Instant::now() >= deadline {
                warn!(
                    remaining,
                    "shutdown timed out before all order cancellations were confirmed"
                );
                return;
            }
            time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn live_symbol(event: &LiveEvent) -> Option<&str> {
    match event {
        LiveEvent::Feed { symbol, .. }
        | LiveEvent::Order { symbol, .. }
        | LiveEvent::Position { symbol, .. } => Some(symbol.as_str()),
        LiveEvent::BatchStart | LiveEvent::BatchEnd | LiveEvent::Error(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use hftbacktest::{
        strategy::{BuiltinStrategy, BuiltinStrategyConfig, GridConfig},
        types::{Event, LOCAL_ASK_DEPTH_EVENT, LOCAL_BID_DEPTH_EVENT, LiveEvent, Order},
    };
    use tokio::{sync::mpsc::UnboundedSender, time};

    use super::{AccountReadiness, LiveService, StrategyRuntimes};
    use crate::{
        live::{risk::RiskConfig, runtime::LiveStrategyRuntime},
        ports::{ExecutionVenue, MarketDataSource, PublishEvent, RunMode, TradingInstrument},
    };

    #[derive(Clone, Default)]
    struct PositionOnlyConnector {
        submitted: Arc<Mutex<Vec<Order>>>,
        snapshot_ready: bool,
    }

    impl MarketDataSource for PositionOnlyConnector {
        fn register(&mut self, _symbol: String) {}
        fn start_market_data(&mut self, _tx: UnboundedSender<PublishEvent>) {}
    }

    impl ExecutionVenue for PositionOnlyConnector {
        fn start_account_stream(
            &self,
            _instruments: Vec<TradingInstrument>,
            tx: UnboundedSender<PublishEvent>,
        ) {
            tx.send(PublishEvent::LiveEvent(LiveEvent::Position {
                symbol: "btcusdt".to_string(),
                qty: 0.0,
                exch_ts: 1,
            }))
            .unwrap();
            if self.snapshot_ready {
                tx.send(PublishEvent::AccountSnapshotReady {
                    symbol: "btcusdt".to_string(),
                })
                .unwrap();
            }
            for (ev, px) in [
                (LOCAL_BID_DEPTH_EVENT, 100.0),
                (LOCAL_ASK_DEPTH_EVENT, 101.0),
            ] {
                tx.send(PublishEvent::LiveEvent(LiveEvent::Feed {
                    symbol: "btcusdt".to_string(),
                    event: Event {
                        ev,
                        exch_ts: 1,
                        local_ts: 1,
                        px,
                        qty: 10.0,
                        order_id: 0,
                        ival: 0,
                        fval: 0.0,
                    },
                }))
                .unwrap();
            }
            tx.send(PublishEvent::BatchEnd).unwrap();
        }

        fn open_orders(&self, _symbol: &str) -> Vec<Order> {
            Vec::new()
        }

        fn submit(&self, _symbol: String, order: Order, _tx: UnboundedSender<PublishEvent>) {
            self.submitted.lock().unwrap().push(order);
        }

        fn cancel(&self, _symbol: String, _order: Order, _tx: UnboundedSender<PublishEvent>) {}
    }

    fn grid_runtimes() -> StrategyRuntimes {
        let strategy = BuiltinStrategy::from_config(BuiltinStrategyConfig::Grid(GridConfig {
            relative_half_spread: 0.0005,
            relative_grid_interval: 0.0005,
            grid_num: 1,
            min_grid_step: 0.1,
            skew: 0.00025,
            order_qty: 0.001,
            max_position: 0.003,
        }))
        .unwrap();
        let mut runtimes = StrategyRuntimes::new();
        runtimes.insert(
            "btcusdt".to_string(),
            LiveStrategyRuntime::new("btcusdt", 0.1, 0.001, strategy),
        );
        runtimes
    }

    #[test]
    fn account_disconnect_invalidates_every_reconciled_symbol() {
        let mut readiness = AccountReadiness::default();
        readiness.reconcile("btcusdt");
        readiness.reconcile("ethusdt");
        assert!(readiness.contains("btcusdt"));
        assert!(readiness.contains("ethusdt"));

        readiness.disconnect();
        assert!(!readiness.contains("btcusdt"));
        assert!(!readiness.contains("ethusdt"));

        readiness.reconcile("btcusdt");
        assert!(readiness.contains("btcusdt"));
        assert!(!readiness.contains("ethusdt"));
    }

    #[tokio::test]
    async fn position_without_order_recovery_never_enables_execution() {
        let connector = PositionOnlyConnector::default();
        let submitted = connector.submitted.clone();
        let service = LiveService::new(
            connector,
            grid_runtimes(),
            RiskConfig::default(),
            RunMode::Execute,
        );

        service
            .run_until(async {
                time::sleep(Duration::from_millis(20)).await;
            })
            .await
            .unwrap();

        assert!(
            submitted.lock().unwrap().is_empty(),
            "a position snapshot alone must not authorize order submission"
        );
    }

    #[tokio::test]
    async fn completed_account_snapshot_enables_execution() {
        let connector = PositionOnlyConnector {
            snapshot_ready: true,
            ..PositionOnlyConnector::default()
        };
        let submitted = connector.submitted.clone();
        let service = LiveService::new(
            connector,
            grid_runtimes(),
            RiskConfig::default(),
            RunMode::Execute,
        );

        service
            .run_until(async {
                time::sleep(Duration::from_millis(20)).await;
            })
            .await
            .unwrap();

        assert!(!submitted.lock().unwrap().is_empty());
    }
}
