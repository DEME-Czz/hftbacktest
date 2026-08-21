use hftbacktest::{
    strategy::{BuiltinStrategy, StrategyCommand},
    types::Side,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use crate::{
    connector::{Connector, PublishEvent},
    risk::RiskGate,
    runtime::LiveStrategyRuntime,
};

pub struct LiveExecutor {
    execute: bool,
    risk: RiskGate,
}

impl LiveExecutor {
    pub fn new(execute: bool, risk: RiskGate) -> Self { Self { execute, risk } }

    pub fn execute<C: Connector>(
        &self,
        connector: &C,
        tx: &UnboundedSender<PublishEvent>,
        runtime: &mut LiveStrategyRuntime<BuiltinStrategy>,
        commands: Vec<StrategyCommand>,
    ) {
        for command in commands {
            if let Err(reason) = self.risk.allow(&command, runtime.position(), runtime.open_orders()) {
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
                    // Current Binance Connector contract deliberately exposes submit/cancel only.
                    // Keep this explicit so future strategies cannot silently assume live modify.
                    warn!(symbol = runtime.symbol(), order_id, "live modify is not implemented; command rejected");
                }
            }
        }
    }
}
