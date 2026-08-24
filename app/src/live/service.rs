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
const SAFETY_CANCEL_RETRY: Duration = Duration::from_millis(250);

#[derive(Default)]
struct AccountReadiness {
    symbols: HashSet<String>,
    halted: HashSet<String>,
    recovered_snapshots: HashSet<String>,
    position_timestamps: HashMap<String, i64>,
    pending_position_after_fill: HashMap<String, i64>,
}

impl AccountReadiness {
    fn reconcile(&mut self, symbol: &str) -> bool {
        if self.halted.contains(symbol)
            || !self.position_timestamps.contains_key(symbol)
        {
            return false;
        }
        self.recovered_snapshots.insert(symbol.to_string());
        if self.pending_position_after_fill.contains_key(symbol) {
            return false;
        }
        self.symbols.insert(symbol.to_string())
    }

    fn observe_position(&mut self, symbol: &str, exch_ts: i64, applied: bool) {
        if !applied {
            return;
        }
        self.position_timestamps
            .entry(symbol.to_string())
            .and_modify(|current| *current = (*current).max(exch_ts))
            .or_insert(exch_ts);
        let position_is_current = self
            .pending_position_after_fill
            .get(symbol)
            .is_some_and(|required| exch_ts >= *required);
        if position_is_current {
            self.pending_position_after_fill.remove(symbol);
            if self.recovered_snapshots.contains(symbol) && !self.halted.contains(symbol) {
                self.symbols.insert(symbol.to_string());
            }
        }
    }

    fn observe_order(&mut self, symbol: &str, order: &hftbacktest::types::Order) {
        if !matches!(
            order.status,
            hftbacktest::types::Status::PartiallyFilled | hftbacktest::types::Status::Filled
        ) || !order.exec_qty.is_finite()
            || order.exec_qty <= 0.0
        {
            return;
        }
        let position_is_current = self
            .position_timestamps
            .get(symbol)
            .is_some_and(|position_ts| *position_ts >= order.exch_timestamp);
        if position_is_current {
            return;
        }
        self.symbols.remove(symbol);
        self.pending_position_after_fill
            .entry(symbol.to_string())
            .and_modify(|required| *required = (*required).max(order.exch_timestamp))
            .or_insert(order.exch_timestamp);
    }

    fn disconnect(&mut self) {
        self.symbols.clear();
        self.recovered_snapshots.clear();
        self.position_timestamps.clear();
        self.pending_position_after_fill.clear();
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
        self.run_until(shutdown_signal()).await
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
        let mut cancellation_requests = HashMap::new();
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
                    return self.cancel_before_shutdown(&tx).await;
                }
                event = rx.recv() => match event {
                    Some(PublishEvent::LiveEvent(live)) => {
                        trace!(?live, "runtime event");
                        if let Some(symbol) = live_symbol(&live)
                            && let Some(runtime) = self.runtimes.get_mut(symbol)
                        {
                            let applied = runtime.apply(&live);
                            match &live {
                                LiveEvent::Position { exch_ts, .. } => {
                                    account_readiness.observe_position(symbol, *exch_ts, applied);
                                }
                                LiveEvent::Order { order, .. } if applied => {
                                    account_readiness.observe_order(symbol, order);
                                }
                                _ => {}
                            }
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

    async fn cancel_before_shutdown(
        &self,
        tx: &tokio::sync::mpsc::UnboundedSender<PublishEvent>,
    ) -> Result<()> {
        if !self.mode.allows_trading() {
            return Ok(());
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
            return Ok(());
        }

        info!(
            count = active_orders.len(),
            "canceling active strategy orders before shutdown"
        );
        for (symbol, order) in active_orders {
            self.connector.cancel(symbol, order, tx.clone());
        }

        let deadline =
            time::Instant::now() + Duration::from_millis(self.safety.shutdown_cancel_timeout_ms);
        loop {
            let remaining = self
                .runtimes
                .keys()
                .map(|symbol| self.connector.open_orders(symbol).len())
                .sum::<usize>();
            if remaining == 0 {
                return Ok(());
            }
            if time::Instant::now() >= deadline {
                bail!("shutdown timed out with {remaining} unconfirmed strategy order(s)");
            }
            time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn cancel_safety_orders(
        &self,
        safety: &SafetyState,
        tx: &tokio::sync::mpsc::UnboundedSender<PublishEvent>,
        requested: &mut HashMap<(String, u64), time::Instant>,
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
                requested.retain(|(requested_symbol, _), _| requested_symbol != symbol);
                continue;
            }
            let active_ids: HashSet<_> = orders.iter().map(|order| order.order_id).collect();
            requested.retain(|(requested_symbol, order_id), _| {
                requested_symbol != symbol || active_ids.contains(order_id)
            });
            for order in orders {
                let request_key = (symbol.clone(), order.order_id);
                let now = time::Instant::now();
                let retry_due = requested
                    .get(&request_key)
                    .is_none_or(|last_request| now.duration_since(*last_request) >= SAFETY_CANCEL_RETRY);
                if retry_due {
                    requested.insert(request_key, now);
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

#[cfg(unix)]
async fn shutdown_signal() {
    let Ok(mut terminate) = signal::unix::signal(signal::unix::SignalKind::terminate()) else {
        error!("failed to install SIGTERM handler; waiting for SIGINT only");
        let _ = signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = signal::ctrl_c().await;
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

    #[derive(Clone)]
    struct PositionOnlyConnector {
        submitted: Arc<Mutex<Vec<Order>>>,
        open_orders: Arc<Mutex<Vec<Order>>>,
        canceled: Arc<Mutex<Vec<Order>>>,
        snapshot_ready: bool,
        confirm_cancel: bool,
    }

    #[derive(Clone, Default)]
    struct ManualConnector {
        event_tx: Arc<Mutex<Option<UnboundedSender<PublishEvent>>>>,
        submitted: Arc<Mutex<Vec<Order>>>,
    }

    impl MarketDataSource for ManualConnector {
        fn register(&mut self, _symbol: String) {}

        fn start_market_data(&mut self, tx: UnboundedSender<PublishEvent>) {
            *self.event_tx.lock().unwrap() = Some(tx);
        }
    }

    impl ExecutionVenue for ManualConnector {
        fn start_account_stream(
            &self,
            _instruments: Vec<TradingInstrument>,
            _tx: UnboundedSender<PublishEvent>,
        ) {
        }

        fn open_orders(&self, _symbol: &str) -> Vec<Order> {
            Vec::new()
        }

        fn submit(
            &self,
            _symbol: String,
            order: Order,
            _lot_size: f64,
            _tx: UnboundedSender<PublishEvent>,
        ) {
            self.submitted.lock().unwrap().push(order);
        }

        fn cancel(&self, _symbol: String, _order: Order, _tx: UnboundedSender<PublishEvent>) {}
    }

    fn send_depth_batch(tx: &UnboundedSender<PublishEvent>, timestamp: i64, quantity: f64) {
        for (ev, px) in [
            (LOCAL_BID_DEPTH_EVENT, 100.0),
            (LOCAL_ASK_DEPTH_EVENT, 101.0),
        ] {
            tx.send(PublishEvent::LiveEvent(LiveEvent::Feed {
                symbol: "btcusdt".to_string(),
                event: Event {
                    ev,
                    exch_ts: timestamp,
                    local_ts: timestamp,
                    px,
                    qty: quantity,
                    order_id: 0,
                    ival: 0,
                    fval: 0.0,
                },
            }))
            .unwrap();
        }
        tx.send(PublishEvent::BatchEnd).unwrap();
    }

    impl Default for PositionOnlyConnector {
        fn default() -> Self {
            Self {
                submitted: Default::default(),
                open_orders: Default::default(),
                canceled: Default::default(),
                snapshot_ready: false,
                confirm_cancel: true,
            }
        }
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

        fn submit(
            &self,
            _symbol: String,
            order: Order,
            _lot_size: f64,
            _tx: UnboundedSender<PublishEvent>,
        ) {
            self.submitted.lock().unwrap().push(order);
        }

        fn cancel(&self, _symbol: String, order: Order, _tx: UnboundedSender<PublishEvent>) {
            if self.confirm_cancel {
                self.open_orders
                    .lock()
                    .unwrap()
                    .retain(|open| open.order_id != order.order_id);
            }
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
        readiness.observe_position("btcusdt", 1, true);
        readiness.observe_position("ethusdt", 1, true);
        readiness.reconcile("btcusdt");
        readiness.reconcile("ethusdt");
        assert!(readiness.contains("btcusdt"));
        assert!(readiness.contains("ethusdt"));

        readiness.disconnect();
        assert!(!readiness.contains("btcusdt"));
        assert!(!readiness.contains("ethusdt"));

        readiness.observe_position("btcusdt", 2, true);
        readiness.reconcile("btcusdt");
        assert!(readiness.contains("btcusdt"));
        assert!(!readiness.contains("ethusdt"));
    }

    #[test]
    fn account_deltas_cannot_bypass_the_startup_snapshot_barrier() {
        let mut readiness = AccountReadiness::default();
        let mut fill = Order::new(
            11,
            1_000,
            0.1,
            0.001,
            hftbacktest::types::Side::Buy,
            hftbacktest::types::OrdType::Limit,
            hftbacktest::types::TimeInForce::GTC,
        );
        fill.status = hftbacktest::types::Status::Filled;
        fill.exec_qty = fill.qty;
        fill.exch_timestamp = 200;

        readiness.observe_order("btcusdt", &fill);
        readiness.observe_position("btcusdt", 200, true);

        assert!(
            !readiness.contains("btcusdt"),
            "account deltas alone must not authorize execution before recovery is ready"
        );
    }

    #[test]
    fn canceled_order_with_historical_fills_does_not_wait_for_a_new_position_event() {
        let mut readiness = AccountReadiness::default();
        readiness.observe_position("btcusdt", 100, true);
        assert!(readiness.reconcile("btcusdt"));
        let mut canceled = Order::new(
            12,
            1_000,
            0.1,
            0.001,
            hftbacktest::types::Side::Buy,
            hftbacktest::types::OrdType::Limit,
            hftbacktest::types::TimeInForce::GTC,
        );
        canceled.status = hftbacktest::types::Status::Canceled;
        canceled.exec_qty = 0.0005;
        canceled.exch_timestamp = 200;

        readiness.observe_order("btcusdt", &canceled);

        assert!(readiness.contains("btcusdt"));
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
    async fn fill_blocks_replenishment_until_the_matching_position_update_arrives() {
        let connector = ManualConnector::default();
        let event_tx = connector.event_tx.clone();
        let submitted = connector.submitted.clone();
        let service = LiveService::new(
            connector,
            grid_runtimes(),
            RiskConfig::default(),
            RunMode::Execute,
        );

        service
            .run_until(async move {
                let tx = loop {
                    if let Some(tx) = event_tx.lock().unwrap().clone() {
                        break tx;
                    }
                    tokio::task::yield_now().await;
                };
                tx.send(PublishEvent::LiveEvent(LiveEvent::Position {
                    symbol: "btcusdt".to_string(),
                    qty: 0.0,
                    exch_ts: 100,
                }))
                .unwrap();
                tx.send(PublishEvent::AccountSnapshotReady {
                    symbol: "btcusdt".to_string(),
                })
                .unwrap();
                send_depth_batch(&tx, 100, 10.0);
                time::sleep(Duration::from_millis(20)).await;

                let (initial_count, mut filled) = {
                    let submitted = submitted.lock().unwrap();
                    let filled = submitted
                        .iter()
                        .find(|order| order.side == hftbacktest::types::Side::Buy)
                        .cloned()
                        .expect("initial grid must include a bid");
                    (submitted.len(), filled)
                };
                filled.req = hftbacktest::types::Status::None;
                filled.status = hftbacktest::types::Status::Filled;
                filled.exec_qty = filled.qty;
                filled.leaves_qty = 0.0;
                filled.exch_timestamp = 200;
                tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                    symbol: "btcusdt".to_string(),
                    order: filled,
                }))
                .unwrap();
                send_depth_batch(&tx, 200, 11.0);
                time::sleep(Duration::from_millis(20)).await;

                assert_eq!(
                    submitted.lock().unwrap().len(),
                    initial_count,
                    "a fill must not be replenished while position is still stale"
                );

                tx.send(PublishEvent::LiveEvent(LiveEvent::Position {
                    symbol: "btcusdt".to_string(),
                    qty: 0.001,
                    exch_ts: 200,
                }))
                .unwrap();
            })
            .await
            .unwrap();
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
                ..SafetyConfig::default()
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
    async fn stale_market_retries_cancellation_while_the_order_remains_active() {
        let connector = PositionOnlyConnector {
            confirm_cancel: false,
            ..PositionOnlyConnector::default()
        };
        let mut order = Order::new(
            13,
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
                shutdown_cancel_timeout_ms: 25,
                kill_switch_file: None,
            },
            RunMode::Execute,
        );

        let result = service
            .run_until(async {
                time::sleep(Duration::from_millis(560)).await;
            })
            .await;

        assert!(result.is_err(), "the unconfirmed order must fail shutdown");
        assert!(
            canceled.lock().unwrap().len() >= 3,
            "stale cancellation must retry before the final shutdown attempt"
        );
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
                ..SafetyConfig::default()
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

    #[tokio::test]
    async fn shutdown_with_unconfirmed_orders_returns_an_error() {
        let connector = PositionOnlyConnector {
            confirm_cancel: false,
            ..PositionOnlyConnector::default()
        };
        let mut order = Order::new(
            9,
            1_000,
            0.1,
            0.001,
            hftbacktest::types::Side::Buy,
            hftbacktest::types::OrdType::Limit,
            hftbacktest::types::TimeInForce::GTC,
        );
        order.status = hftbacktest::types::Status::New;
        connector.open_orders.lock().unwrap().push(order);
        let service = LiveService::with_safety(
            connector,
            grid_runtimes(),
            RiskConfig::default(),
            SafetyConfig {
                shutdown_cancel_timeout_ms: 25,
                ..SafetyConfig::default()
            },
            RunMode::Execute,
        );

        let result = service.run_until(async {}).await;

        assert!(result.is_err());
    }
}
