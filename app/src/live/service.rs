use std::{
    collections::{HashMap, HashSet},
    future::Future,
    time::Duration,
};

use anyhow::{Result, bail};
use hftbacktest::{strategy::BuiltinStrategy, types::LiveEvent};
use tokio::{select, signal, sync::mpsc::unbounded_channel, time};
use tracing::{info, trace, warn};

use super::{
    config::LiveStrategyConfig,
    execution::LiveExecutor,
    risk::{RiskConfig, RiskGate},
    runtime::LiveStrategyRuntime,
};
use crate::ports::{LiveConnector, PublishEvent, RunMode};

pub type StrategyRuntimes = HashMap<String, LiveStrategyRuntime<BuiltinStrategy>>;

#[derive(Default)]
struct AccountReadiness {
    symbols: HashSet<String>,
}

impl AccountReadiness {
    fn reconcile(&mut self, symbol: &str) -> bool {
        self.symbols.insert(symbol.to_string())
    }

    fn disconnect(&mut self) {
        self.symbols.clear();
    }

    fn contains(&self, symbol: &str) -> bool {
        self.symbols.contains(symbol)
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
            self.connector.start_account_stream(tx.clone());
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
                        if let LiveEvent::Position { symbol, .. } = &live
                            && self.runtimes.contains_key(symbol)
                            && account_readiness.reconcile(symbol)
                        {
                            info!(%symbol, "initial position state synchronized");
                        }
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
    use super::AccountReadiness;

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
}
