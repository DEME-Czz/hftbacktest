mod local;
mod nopartialfillexchange;
mod partialfillexchange;

use std::collections::HashMap;

pub use local::Local;
pub use nopartialfillexchange::NoPartialFillExchange;
pub use partialfillexchange::PartialFillExchange;

use crate::{
    backtest::BacktestError,
    depth::MarketDepth,
    prelude::{Event, OrdType, Order, OrderId, Side, StateValues, TimeInForce},
};

pub trait LocalProcessor<MD>: Processor
where
    MD: MarketDepth,
{
    #[allow(clippy::too_many_arguments)]
    fn submit_order(
        &mut self,
        order_id: OrderId,
        side: Side,
        price: f64,
        qty: f64,
        order_type: OrdType,
        time_in_force: TimeInForce,
        current_timestamp: i64,
    ) -> Result<(), BacktestError>;

    fn modify(
        &mut self,
        order_id: OrderId,
        price: f64,
        qty: f64,
        current_timestamp: i64,
    ) -> Result<(), BacktestError>;

    fn cancel(&mut self, order_id: OrderId, current_timestamp: i64) -> Result<(), BacktestError>;
    fn clear_inactive_orders(&mut self);
    fn position(&self) -> f64;
    fn state_values(&self) -> &StateValues;
    fn depth(&self) -> &MD;
    fn orders(&self) -> &HashMap<OrderId, Order>;
    fn last_trades(&self) -> &[Event];
    fn clear_last_trades(&mut self);
    fn feed_latency(&self) -> Option<(i64, i64)>;
    fn order_latency(&self) -> Option<(i64, i64, i64)>;
}

impl<P: Processor + ?Sized> Processor for Box<P> {
    fn event_seen_timestamp(&self, event: &Event) -> Option<i64> {
        P::event_seen_timestamp(self, event)
    }
    fn process(&mut self, event: &Event) -> Result<(), BacktestError> {
        P::process(self, event)
    }
    fn process_recv_order(
        &mut self,
        timestamp: i64,
        wait_resp_order_id: Option<OrderId>,
    ) -> Result<bool, BacktestError> {
        P::process_recv_order(self, timestamp, wait_resp_order_id)
    }
    fn earliest_recv_order_timestamp(&self) -> i64 {
        P::earliest_recv_order_timestamp(self)
    }
    fn earliest_send_order_timestamp(&self) -> i64 {
        P::earliest_send_order_timestamp(self)
    }
}

pub trait Processor {
    fn event_seen_timestamp(&self, event: &Event) -> Option<i64>;
    fn process(&mut self, event: &Event) -> Result<(), BacktestError>;
    fn process_recv_order(
        &mut self,
        timestamp: i64,
        wait_resp_order_id: Option<OrderId>,
    ) -> Result<bool, BacktestError>;
    fn earliest_recv_order_timestamp(&self) -> i64;
    fn earliest_send_order_timestamp(&self) -> i64;
}
