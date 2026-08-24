use hftbacktest::{
    strategy::{BuiltinStrategy, StrategyCommand},
    types::Side,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use crate::ports::{ExecutionVenue, PublishEvent};

use super::{risk::RiskGate, runtime::LiveStrategyRuntime};

pub struct LiveExecutor {
    execute: bool,
    risk: RiskGate,
}

impl LiveExecutor {
    pub fn new(execute: bool, risk: RiskGate) -> Self { Self { execute, risk } }

    pub fn execute<C: ExecutionVenue>(
        &self,
        connector: &C,
        tx: &UnboundedSender<PublishEvent>,
        runtime: &mut LiveStrategyRuntime<BuiltinStrategy>,
        commands: Vec<StrategyCommand>,
    ) {
        for command in commands {
            let same_side_order_exposure = match &command {
                StrategyCommand::Submit { side, .. } => runtime.active_order_exposure(*side),
                StrategyCommand::Modify { .. } | StrategyCommand::Cancel { .. } => 0.0,
            };
            if let Err(reason) = self.risk.allow(
                &command,
                runtime.position(),
                runtime.open_orders(),
                same_side_order_exposure,
            ) {
                warn!(symbol = runtime.symbol(), ?command, reason, "strategy command rejected by risk gate");
                continue;
            }

            if !self.execute {
                debug!(symbol = runtime.symbol(), ?command, "strategy command generated (dry-run)");
                continue;
            }

            match command {
                StrategyCommand::Submit {
                    order_id,
                    price,
                    qty,
                    side,
                    time_in_force,
                    order_type,
                } => {
                    if !matches!(side, Side::Buy | Side::Sell) {
                        warn!(symbol = runtime.symbol(), order_id, "unsupported order side");
                        continue;
                    }
                    let order = runtime.stage_submit(
                        order_id,
                        price,
                        qty,
                        side,
                        time_in_force,
                        order_type,
                    );
                    connector.submit(runtime.symbol().to_string(), order, tx.clone());
                }
                StrategyCommand::Cancel { order_id } => {
                    if let Some(order) = runtime.stage_cancel(order_id) {
                        connector.cancel(runtime.symbol().to_string(), order, tx.clone());
                    }
                }
                StrategyCommand::Modify { order_id, .. } => {
                    warn!(symbol = runtime.symbol(), order_id, "live modify is not implemented; command rejected");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use hftbacktest::{
        strategy::{BuiltinStrategy, BuiltinStrategyConfig, GridConfig, StrategyCommand},
        types::{
            Event, LiveEvent, OrdType, Order, Side, TimeInForce, LOCAL_ASK_DEPTH_EVENT,
            LOCAL_BID_DEPTH_EVENT,
        },
    };
    use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

    use super::LiveExecutor;
    use crate::{
        live::{
            risk::{RiskConfig, RiskGate},
            runtime::LiveStrategyRuntime,
        },
        ports::{ExecutionVenue, PublishEvent},
    };

    #[derive(Clone, Default)]
    struct FakeConnector {
        submitted: Arc<Mutex<Vec<(String, Order)>>>,
        canceled: Arc<Mutex<Vec<(String, Order)>>>,
    }

    impl ExecutionVenue for FakeConnector {
        fn start_account_stream(&self, _tx: UnboundedSender<PublishEvent>) {}
        fn open_orders(&self, _symbol: &str) -> Vec<Order> { Vec::new() }
        fn submit(&self, symbol: String, order: Order, _tx: UnboundedSender<PublishEvent>) {
            self.submitted.lock().unwrap().push((symbol, order));
        }
        fn cancel(&self, symbol: String, order: Order, _tx: UnboundedSender<PublishEvent>) {
            self.canceled.lock().unwrap().push((symbol, order));
        }
    }

    fn runtime() -> LiveStrategyRuntime<BuiltinStrategy> {
        let strategy = BuiltinStrategy::from_config(BuiltinStrategyConfig::Grid(GridConfig {
            relative_half_spread: 0.0005,
            relative_grid_interval: 0.0005,
            grid_num: 2,
            min_grid_step: 0.1,
            skew: 0.00025,
            order_qty: 0.001,
            max_position: 0.003,
        }))
        .unwrap();
        let mut runtime = LiveStrategyRuntime::new("btcusdt", 0.1, 0.001, strategy);
        runtime.apply(&LiveEvent::Feed {
            symbol: "btcusdt".to_string(),
            event: Event {
                ev: LOCAL_BID_DEPTH_EVENT,
                exch_ts: 1,
                local_ts: 1,
                px: 100.0,
                qty: 10.0,
                order_id: 0,
                ival: 0,
                fval: 0.0,
            },
        });
        runtime.apply(&LiveEvent::Feed {
            symbol: "btcusdt".to_string(),
            event: Event {
                ev: LOCAL_ASK_DEPTH_EVENT,
                exch_ts: 1,
                local_ts: 1,
                px: 101.0,
                qty: 10.0,
                order_id: 0,
                ival: 0,
                fval: 0.0,
            },
        });
        runtime
    }

    #[test]
    fn execute_stages_orders_and_prevents_duplicate_grid_submits() {
        let connector = FakeConnector::default();
        let (tx, _rx) = unbounded_channel();
        let executor = LiveExecutor::new(
            true,
            RiskGate::new(RiskConfig {
                max_order_qty: 0.001,
                max_order_notional: 1.0,
                max_position: 0.003,
                max_open_orders: 4,
            }),
        );
        let mut runtime = runtime();
        let commands = runtime.decide();
        assert!(!commands.is_empty());
        executor.execute(&connector, &tx, &mut runtime, commands);
        assert_eq!(connector.submitted.lock().unwrap().len(), 4);
        assert_eq!(runtime.open_orders(), 4);

        let next = runtime.decide();
        assert!(next.is_empty());
    }

    #[test]
    fn dry_run_never_submits_orders() {
        let connector = FakeConnector::default();
        let (tx, _rx) = unbounded_channel();
        let executor = LiveExecutor::new(false, RiskGate::new(RiskConfig::default()));
        let mut runtime = runtime();
        let commands = runtime.decide();
        executor.execute(&connector, &tx, &mut runtime, commands);
        assert!(connector.submitted.lock().unwrap().is_empty());
        assert_eq!(runtime.open_orders(), 0);
    }

    #[test]
    fn active_order_exposure_counts_toward_position_limit() {
        let connector = FakeConnector::default();
        let (tx, _rx) = unbounded_channel();
        let executor = LiveExecutor::new(
            true,
            RiskGate::new(RiskConfig {
                max_order_qty: 0.01,
                max_order_notional: 1_000.0,
                max_position: 0.003,
                max_open_orders: 4,
            }),
        );
        let mut runtime = runtime();
        runtime.stage_submit(
            1,
            99.0,
            0.0025,
            Side::Buy,
            TimeInForce::GTX,
            OrdType::Limit,
        );

        executor.execute(
            &connector,
            &tx,
            &mut runtime,
            vec![StrategyCommand::Submit {
                order_id: 2,
                price: 98.0,
                qty: 0.001,
                side: Side::Buy,
                time_in_force: TimeInForce::GTX,
                order_type: OrdType::Limit,
            }],
        );

        assert!(
            connector.submitted.lock().unwrap().is_empty(),
            "same-side active order quantity must be included in max_position"
        );
    }
}
