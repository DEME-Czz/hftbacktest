use std::collections::{HashMap, VecDeque};

use hftbacktest::{
    depth::{HashMapMarketDepth, L2MarketDepth, MarketDepth},
    strategy::{MarketContext, Strategy, StrategyCommand},
    types::{
        BUY_EVENT, DEPTH_CLEAR_EVENT, DEPTH_EVENT, Event, LiveEvent, OrdType, Order, OrderId,
        SELL_EVENT, Side, Status, TRADE_EVENT, TimeInForce,
    },
};

pub use super::config::{LiveStrategyConfig, RuntimeConfig, StrategyConfig};

/// In-process normalized market/account state used by live strategies.
pub struct LiveStrategyRuntime<S> {
    symbol: String,
    depth: HashMapMarketDepth,
    position: f64,
    orders: HashMap<OrderId, Order>,
    last_trades: VecDeque<Event>,
    strategy: S,
    timestamp: i64,
    position_exch_timestamp: i64,
    depth_dirty: bool,
    quote_dirty: bool,
}

impl<S> LiveStrategyRuntime<S>
where
    S: Strategy<HashMapMarketDepth>,
{
    pub fn new(symbol: impl Into<String>, tick_size: f64, lot_size: f64, strategy: S) -> Self {
        Self {
            symbol: symbol.into().to_lowercase(),
            depth: HashMapMarketDepth::new(tick_size, lot_size),
            position: 0.0,
            orders: HashMap::new(),
            last_trades: VecDeque::with_capacity(1024),
            strategy,
            timestamp: 0,
            position_exch_timestamp: i64::MIN,
            depth_dirty: false,
            quote_dirty: false,
        }
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub fn apply(&mut self, live: &LiveEvent) -> bool {
        match live {
            LiveEvent::Feed { symbol, event } if symbol == &self.symbol => {
                self.timestamp = event.local_ts;
                if event.is(DEPTH_EVENT | BUY_EVENT) {
                    self.depth
                        .update_bid_depth(event.px, event.qty, event.local_ts);
                    self.depth_dirty = true;
                    self.quote_dirty = true;
                    false
                } else if event.is(DEPTH_EVENT | SELL_EVENT) {
                    self.depth
                        .update_ask_depth(event.px, event.qty, event.local_ts);
                    self.depth_dirty = true;
                    self.quote_dirty = true;
                    false
                } else if event.is(DEPTH_CLEAR_EVENT) {
                    self.depth.clear_depth(Side::None, 0.0);
                    self.depth_dirty = true;
                    self.quote_dirty = true;
                    false
                } else if event.is(TRADE_EVENT) {
                    if self.last_trades.len() == 1024 {
                        self.last_trades.pop_front();
                    }
                    self.last_trades.push_back(event.clone());
                    true
                } else {
                    false
                }
            }
            LiveEvent::Order { symbol, order } if symbol == &self.symbol => {
                let materially_changed = self.orders.get(&order.order_id).is_none_or(|existing| {
                    existing.status != order.status
                        || existing.side != order.side
                        || existing.price_tick != order.price_tick
                        || existing.qty != order.qty
                        || existing.leaves_qty != order.leaves_qty
                        || existing.exec_qty != order.exec_qty
                });
                if order.active() || order.pending() {
                    self.orders.insert(order.order_id, order.clone());
                } else {
                    self.orders.remove(&order.order_id);
                }
                if materially_changed {
                    self.quote_dirty = true;
                }
                true
            }
            LiveEvent::Position {
                symbol,
                qty,
                exch_ts,
            } if symbol == &self.symbol => {
                if !qty.is_finite() || *exch_ts < self.position_exch_timestamp {
                    return false;
                }
                if self.position != *qty {
                    self.position = *qty;
                    self.quote_dirty = true;
                }
                self.position_exch_timestamp = *exch_ts;
                true
            }
            _ => false,
        }
    }

    /// Returns true once for each depth batch that changed this symbol.
    pub fn take_depth_dirty(&mut self) -> bool {
        std::mem::take(&mut self.depth_dirty)
    }

    /// Quote dirtiness is independent of depth dirtiness. Fills, order terminal updates and
    /// position changes must also be able to trigger a fresh inventory-aware quote decision.
    pub fn quote_dirty(&self) -> bool {
        self.quote_dirty
    }

    pub fn take_quote_dirty(&mut self) -> bool {
        std::mem::take(&mut self.quote_dirty)
    }

    pub fn decide(&mut self) -> Vec<StrategyCommand> {
        let last_trades = self.last_trades.make_contiguous();
        let context = MarketContext {
            timestamp: self.timestamp,
            depth: &self.depth,
            position: self.position,
            orders: &self.orders,
            last_trades,
        };
        self.strategy.on_event(&context)
    }

    pub fn stage_submit(
        &mut self,
        order_id: OrderId,
        price: f64,
        qty: f64,
        side: Side,
        time_in_force: TimeInForce,
        order_type: OrdType,
    ) -> Order {
        let price_tick = (price / self.depth.tick_size()).round() as i64;
        let mut order = Order::new(
            order_id,
            price_tick,
            self.depth.tick_size(),
            qty,
            side,
            order_type,
            time_in_force,
        );
        order.local_timestamp = self.timestamp;
        order.req = Status::New;
        self.orders.insert(order_id, order.clone());
        order
    }

    pub fn stage_modify(&mut self, order_id: OrderId, price: f64, qty: f64) -> Option<Order> {
        let tick_size = self.depth.tick_size();
        let order = self.orders.get_mut(&order_id)?;
        if !order.cancellable() {
            return None;
        }
        order.price_tick = (price / tick_size).round() as i64;
        order.qty = qty;
        order.leaves_qty = qty;
        order.req = Status::Replaced;
        Some(order.clone())
    }

    pub fn stage_cancel(&mut self, order_id: OrderId) -> Option<Order> {
        let order = self.orders.get_mut(&order_id)?;
        if !order.cancellable() {
            return None;
        }
        order.req = Status::Canceled;
        Some(order.clone())
    }

    pub fn depth(&self) -> &HashMapMarketDepth {
        &self.depth
    }
    pub fn tick_size(&self) -> f64 {
        self.depth.tick_size()
    }
    pub fn lot_size(&self) -> f64 {
        self.depth.lot_size()
    }
    pub fn position(&self) -> f64 {
        self.position
    }
    pub fn open_orders(&self) -> usize {
        self.orders.len()
    }
    pub fn active_order_exposure(&self, side: Side) -> f64 {
        self.orders
            .values()
            .filter(|order| order.side == side && (order.active() || order.pending()))
            .map(|order| order.leaves_qty)
            .sum()
    }
}

pub struct NoopStrategy;

impl Strategy<HashMapMarketDepth> for NoopStrategy {
    fn on_event(
        &mut self,
        _context: &MarketContext<'_, HashMapMarketDepth>,
    ) -> Vec<StrategyCommand> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use hftbacktest::types::{Event, LOCAL_BID_DEPTH_EVENT, LiveEvent, OrdType, Side, TimeInForce};

    use super::{LiveStrategyRuntime, NoopStrategy};

    #[test]
    fn stale_position_update_cannot_roll_back_a_newer_snapshot() {
        let mut runtime = LiveStrategyRuntime::new("btcusdt", 0.1, 0.001, NoopStrategy);
        runtime.apply(&LiveEvent::Position {
            symbol: "btcusdt".to_string(),
            qty: 2.0,
            exch_ts: 200,
        });
        runtime.apply(&LiveEvent::Position {
            symbol: "btcusdt".to_string(),
            qty: 1.0,
            exch_ts: 100,
        });

        assert_eq!(runtime.position(), 2.0);
    }

    #[test]
    fn position_change_marks_quotes_dirty() {
        let mut runtime = LiveStrategyRuntime::new("btcusdt", 0.1, 0.001, NoopStrategy);
        assert!(!runtime.quote_dirty());

        runtime.apply(&LiveEvent::Position {
            symbol: "btcusdt".to_string(),
            qty: 0.5,
            exch_ts: 100,
        });

        assert!(runtime.take_quote_dirty());
        assert!(!runtime.quote_dirty());
    }

    #[test]
    fn staged_order_records_latest_market_timestamp() {
        let mut runtime = LiveStrategyRuntime::new("btcusdt", 0.1, 0.001, NoopStrategy);
        runtime.apply(&LiveEvent::Feed {
            symbol: "btcusdt".to_string(),
            event: Event {
                ev: LOCAL_BID_DEPTH_EVENT,
                exch_ts: 10,
                local_ts: 123_000_000,
                px: 100.0,
                qty: 1.0,
                order_id: 0,
                ival: 0,
                fval: 0.0,
            },
        });

        let order =
            runtime.stage_submit(1, 99.0, 0.001, Side::Buy, TimeInForce::GTX, OrdType::Limit);

        assert_eq!(order.local_timestamp, 123_000_000);
    }
}
