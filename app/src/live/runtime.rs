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
    depth_dirty: bool,
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
            depth_dirty: false,
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
                    false
                } else if event.is(DEPTH_EVENT | SELL_EVENT) {
                    self.depth
                        .update_ask_depth(event.px, event.qty, event.local_ts);
                    self.depth_dirty = true;
                    false
                } else if event.is(DEPTH_CLEAR_EVENT) {
                    self.depth.clear_depth(Side::None, 0.0);
                    self.depth_dirty = true;
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
                if order.active() || order.pending() {
                    self.orders.insert(order.order_id, order.clone());
                } else {
                    self.orders.remove(&order.order_id);
                }
                true
            }
            LiveEvent::Position {
                symbol,
                qty,
                exch_ts,
            } if symbol == &self.symbol => {
                self.position = *qty;
                self.timestamp = *exch_ts;
                true
            }
            _ => false,
        }
    }

    /// Returns true once for each depth batch that changed this symbol.
    pub fn take_depth_dirty(&mut self) -> bool {
        std::mem::take(&mut self.depth_dirty)
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
