use std::{
    collections::{HashMap, HashSet},
    fs,
    future::Future,
    io,
    time::Duration,
};

use anyhow::{Result, bail};
use hftbacktest::{strategy::BuiltinStrategy, types::LiveEvent};
use tokio::{select, signal, sync::mpsc::unbounded_channel, time};
use tracing::{error, info, trace, warn};

use super::{
    config::{LiveStrategyConfig, SafetyConfig},
    execution::LiveExecutor,
    risk::{RiskConfig, RiskGate},
    runtime::LiveStrategyRuntime,
    safety::SafetyState,
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
    safety: SafetyConfig,
}

impl<C: LiveConnector> LiveService<C> {
    pub fn new(connector: C, runtimes: StrategyRuntimes, risk: RiskConfig, mode: RunMode) -> Self {
        Self::with_safety(connector, runtimes, risk, SafetyConfig::default(), mode)
    }

    pub fn with_safety(
        connector: C,
        runtimes: StrategyRuntimes,
        risk: RiskConfig,
        safety: SafetyConfig,
        mode: RunMode,
    ) -> Self {
        Self {
            connector,
            runtimes,
            executor: LiveExecutor::new(mode.allows_trading(), RiskGate::new(risk)),
            mode,
            safety,
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
        let started_at = time::Instant::now();
        let mut safety_state = SafetyState::new(
            self.safety.stale_market_timeout_ms,
            self.runtimes.keys().cloned(),
        );
        let safety_tick_ms = (self.safety.stale_market_timeout_ms / 4).clamp(25, 250);
        let mut safety_interval = time::interval(Duration::from_millis(safety_tick_ms));
        safety_interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut cancellation_requests = HashSet::new();
        if kill_switch_is_active(self.safety.kill_switch_file.as_deref()) {
            safety_state.trip_kill_switch();
            error!("external kill switch was active at startup; live execution is latched off");
        }
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
                            let now_ms = elapsed_ms(started_at);
                            let has_open_orders = !self
                                .connector
                                .open_orders(runtime.symbol())
                                .is_empty();
                            safety_state.on_market_batch(
                                runtime.symbol(),
                                now_ms,
                                has_open_orders,
                            );
                            if self.mode.allows_trading()
                                && (!account_readiness.contains(runtime.symbol())
                                    || !safety_state.can_submit(runtime.symbol(), now_ms))
                            {
                                trace!(symbol = runtime.symbol(), "execution waiting for safe market and account state");
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
                        safety_state.mark_disconnected();
                        self.cancel_safety_orders(
                            &safety_state,
                            &tx,
                            &mut cancellation_requests,
                        );
                        warn!("Binance account stream disconnected; active orders are being canceled");
                    }
                    Some(PublishEvent::MarketStreamDisconnected) => {
                        safety_state.mark_disconnected();
                        self.cancel_safety_orders(
                            &safety_state,
                            &tx,
                            &mut cancellation_requests,
                        );
                        warn!("Binance market stream disconnected; active orders are being canceled");
                    }
                    Some(PublishEvent::AccountSnapshotReady { symbol }) => {
                        if self.runtimes.contains_key(&symbol)
                            && account_readiness.reconcile(&symbol)
                        {
                            info!(%symbol, "position and open-order state synchronized");
                        }
                        self.cancel_safety_orders(
                            &safety_state,
                            &tx,
                            &mut cancellation_requests,
                        );
                    }
                    Some(PublishEvent::ExecutionUncertain { symbol }) => {
                        account_readiness.halt(&symbol);
                        safety_state.halt_symbol(&symbol);
                        self.cancel_safety_orders(
                            &safety_state,
                            &tx,
                            &mut cancellation_requests,
                        );
                        error!(%symbol, "execution halted: order submission outcome is unresolved");
                    }
                    None => break,
                },
                _ = safety_interval.tick() => {
                    let now_ms = elapsed_ms(started_at);
                    safety_state.on_tick(now_ms);
                    if !safety_state.kill_latched()
                        && kill_switch_is_active(self.safety.kill_switch_file.as_deref())
                    {
                        safety_state.trip_kill_switch();
                        error!("external kill switch tripped; live execution is latched off");
                    }
                    self.cancel_safety_orders(
                        &safety_state,
                        &tx,
                        &mut cancellation_requests,
                    );
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

    fn cancel_safety_orders(
        &self,
        safety: &SafetyState,
        tx: &tokio::sync::mpsc::UnboundedSender<PublishEvent>,
        requested: &mut HashSet<(String, u64)>,
    ) {
        if !self.mode.allows_trading() {
            return;
        }
        for symbol in self.runtimes.keys() {
            if !safety.requires_cancel(symbol) {
                continue;
            }
            let orders = self.connector.open_orders(symbol);
            if orders.is_empty() {
                requested.retain(|(requested_symbol, _)| requested_symbol != symbol);
                continue;
            }
            for order in orders {
                if requested.insert((symbol.clone(), order.order_id)) {
                    warn!(%symbol, order_id = order.order_id, "canceling order due to live safety halt");
                    self.connector.cancel(symbol.clone(), order, tx.clone());
                }
            }
        }
    }
}

fn elapsed_ms(started_at: time::Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn kill_switch_is_active(path: Option<&std::path::Path>) -> bool {
    let Some(path) = path else {
        return false;
    };
    match fs::metadata(path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            error!(
                ?error,
                ?path,
                "kill switch state could not be read; failing closed"
            );
            true
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
        live::{config::SafetyConfig, risk::RiskConfig, runtime::LiveStrategyRuntime},
        ports::{ExecutionVenue, MarketDataSource, PublishEvent, RunMode, TradingInstrument},
    };

    #[derive(Clone, Default)]
    struct PositionOnlyConnector {
        submitted: Arc<Mutex<Vec<Order>>>,
        open_orders: Arc<Mutex<Vec<Order>>>,
        canceled: Arc<Mutex<Vec<Order>>>,
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
            self.open_orders.lock().unwrap().clone()
        }

        fn submit(&self, _symbol: String, order: Order, _tx: UnboundedSender<PublishEvent>) {
            self.submitted.lock().unwrap().push(order);
        }

        fn cancel(&self, _symbol: String, order: Order, _tx: UnboundedSender<PublishEvent>) {
            self.open_orders
                .lock()
                .unwrap()
                .retain(|open| open.order_id != order.order_id);
            self.canceled.lock().unwrap().push(order);
        }
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

    #[tokio::test]
    async fn stale_market_cancels_each_tracked_order_once() {
        let connector = PositionOnlyConnector::default();
        let mut order = Order::new(
            7,
            1_000,
            0.1,
            0.001,
            hftbacktest::types::Side::Buy,
            hftbacktest::types::OrdType::Limit,
            hftbacktest::types::TimeInForce::GTC,
        );
        order.status = hftbacktest::types::Status::New;
        connector.open_orders.lock().unwrap().push(order);
        let canceled = connector.canceled.clone();
        let service = LiveService::with_safety(
            connector,
            grid_runtimes(),
            RiskConfig::default(),
            SafetyConfig {
                stale_market_timeout_ms: 20,
                kill_switch_file: None,
            },
            RunMode::Execute,
        );

        service
            .run_until(async {
                time::sleep(Duration::from_millis(90)).await;
            })
            .await
            .unwrap();

        assert_eq!(canceled.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn kill_switch_present_at_start_prevents_submit_and_cancels_tracked_orders() {
        let kill_switch = std::env::temp_dir().join(format!(
            "hft-app-kill-switch-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(&kill_switch, b"halt").unwrap();

        let connector = PositionOnlyConnector {
            snapshot_ready: true,
            ..PositionOnlyConnector::default()
        };
        let mut order = Order::new(
            8,
            1_000,
            0.1,
            0.001,
            hftbacktest::types::Side::Sell,
            hftbacktest::types::OrdType::Limit,
            hftbacktest::types::TimeInForce::GTC,
        );
        order.status = hftbacktest::types::Status::New;
        connector.open_orders.lock().unwrap().push(order);
        let submitted = connector.submitted.clone();
        let canceled = connector.canceled.clone();
        let service = LiveService::with_safety(
            connector,
            grid_runtimes(),
            RiskConfig::default(),
            SafetyConfig {
                stale_market_timeout_ms: 5_000,
                kill_switch_file: Some(kill_switch.clone()),
            },
            RunMode::Execute,
        );

        service
            .run_until(async {
                time::sleep(Duration::from_millis(40)).await;
            })
            .await
            .unwrap();
        std::fs::remove_file(kill_switch).unwrap();

        assert!(submitted.lock().unwrap().is_empty());
        assert_eq!(canceled.lock().unwrap().len(), 1);
    }
}
