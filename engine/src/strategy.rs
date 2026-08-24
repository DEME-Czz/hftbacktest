use std::collections::HashMap;

use crate::{
    depth::MarketDepth,
    types::{Bot, Event, OrdType, Order, OrderId, OrderRequest, Side, TimeInForce},
};

pub mod builtin;
pub mod grid;

pub use builtin::{BuiltinStrategy, BuiltinStrategyConfig};
pub use grid::{GridConfig, GridStrategy};

/// Exchange-independent read-only state presented to a strategy.
pub struct MarketContext<'a, MD: MarketDepth> {
    pub timestamp: i64,
    pub depth: &'a MD,
    pub position: f64,
    pub orders: &'a HashMap<OrderId, Order>,
    pub last_trades: &'a [Event],
}

/// Exchange-independent actions emitted by a strategy.
#[derive(Clone, Debug)]
pub enum StrategyCommand {
    Submit {
        order_id: OrderId,
        price: f64,
        qty: f64,
        side: Side,
        time_in_force: TimeInForce,
        order_type: OrdType,
    },
    Modify {
        order_id: OrderId,
        price: f64,
        qty: f64,
    },
    Cancel {
        order_id: OrderId,
    },
}

/// A strategy is a pure decision component: market/account state in, commands out.
///
/// It must not depend on Binance, WebSocket, REST, Tokio, or backtest-specific transport details.
pub trait Strategy<MD: MarketDepth> {
    fn on_event(&mut self, context: &MarketContext<'_, MD>) -> Vec<StrategyCommand>;
}

/// Executes one strategy decision using the same `Bot` semantics used by backtesting.
pub fn run_once<MD, B, S>(bot: &mut B, strategy: &mut S, asset_no: usize) -> Result<(), B::Error>
where
    MD: MarketDepth,
    B: Bot<MD>,
    S: Strategy<MD>,
{
    let commands = {
        let context = MarketContext {
            timestamp: bot.current_timestamp(),
            depth: bot.depth(asset_no),
            position: bot.position(asset_no),
            orders: bot.orders(asset_no),
            last_trades: bot.last_trades(asset_no),
        };
        strategy.on_event(&context)
    };

    for command in commands {
        match command {
            StrategyCommand::Submit {
                order_id,
                price,
                qty,
                side,
                time_in_force,
                order_type,
            } => {
                let _ = bot.submit_order(
                    asset_no,
                    OrderRequest {
                        order_id,
                        price,
                        qty,
                        side,
                        time_in_force,
                        order_type,
                    },
                    false,
                )?;
            }
            StrategyCommand::Modify {
                order_id,
                price,
                qty,
            } => {
                let _ = bot.modify(asset_no, order_id, price, qty, false)?;
            }
            StrategyCommand::Cancel { order_id } => {
                let _ = bot.cancel(asset_no, order_id, false)?;
            }
        }
    }
    Ok(())
}
