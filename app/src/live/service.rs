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
        if self.halted.contains(symbol) || !self.position_timestamps.contains_key(symbol) {
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
            hftbacktest::types::Status::PartiallyFilled
                | hftbacktest::types::Status::Filled
                | hftbacktest::types::Status::Expired
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
        let started_at = time::Instant::now();
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
                    Some(PublishEvent::BatchEnd { received_at }) => {
                        let now_ms = elapsed_ms(started_at);
                        let batch_age_ms = u64::try_from(received_at.elapsed().as_millis())
                            .unwrap_or(u64::MAX);
                        let received_ms = now_ms.saturating_sub(batch_age_ms);
                        for runtime in self.runtimes.values_mut() {
                            if !runtime.take_depth_dirty() {
                                continue;
                            }
                            let has_open_orders = !self
                                .connector
                                .open_orders(runtime.symbol())
                                .is_empty();
                            safety_state.on_market_batch(
                                runtime.symbol(),
                                received_ms,
                                has_open_orders,
                            );
                            if batch_age_ms >= self.safety.stale_market_timeout_ms {
                                safety_state.halt_symbol(runtime.symbol());
                            }
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
                let retry_due = requested.get(&request_key).is_none_or(|last_request| {
                    now.duration_since(*last_request) >= SAFETY_CANCEL_RETRY
                });
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
    use super::AccountReadiness;

    #[test]
    fn account_readiness_requires_position_and_snapshot() {
        let mut readiness = AccountReadiness::default();
        readiness.observe_position("btcusdt", 10, true);
        assert!(!readiness.contains("btcusdt"));
        assert!(readiness.reconcile("btcusdt"));
        assert!(readiness.contains("btcusdt"));
    }
}
