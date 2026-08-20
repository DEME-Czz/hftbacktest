use std::collections::HashMap;

use hftbacktest::{
    depth::{HashMapMarketDepth, L2MarketDepth},
    strategy::{MarketContext, Strategy, StrategyCommand},
    types::{
        BUY_EVENT, DEPTH_CLEAR_EVENT, DEPTH_EVENT, Event, LiveEvent, Order, OrderId, SELL_EVENT,
        Side, TRADE_EVENT,
    },
};

/// In-process normalized market/account state used by live strategies.
pub struct LiveStrategyRuntime<S> {
    symbol: String,
    depth: HashMapMarketDepth,
    position: f64,
    orders: HashMap<OrderId, Order>,
    last_trades: Vec<Event>,
    strategy: S,
    timestamp: i64,
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
            last_trades: Vec::with_capacity(1024),
            strategy,
            timestamp: 0,
        }
    }

    /// Applies a normalized live event. Depth updates only mutate state; call `decide` after
    /// `PublishEvent::BatchEnd` so a strategy observes a complete Binance depth batch.
    pub fn apply(&mut self, live: &LiveEvent) -> bool {
        match live {
            LiveEvent::Feed { symbol, event } if symbol == &self.symbol => {
                self.timestamp = event.local_ts;
                if event.is(DEPTH_EVENT | BUY_EVENT) {
                    self.depth
                        .update_bid_depth(event.px, event.qty, event.local_ts);
                    false
                } else if event.is(DEPTH_EVENT | SELL_EVENT) {
                    self.depth
                        .update_ask_depth(event.px, event.qty, event.local_ts);
                    false
                } else if event.is(DEPTH_CLEAR_EVENT) {
                    self.depth.clear_depth(Side::None, 0.0);
                    false
                } else if event.is(TRADE_EVENT) {
                    self.last_trades.push(event.clone());
                    if self.last_trades.len() > 1024 {
                        self.last_trades.remove(0);
                    }
                    true
                } else {
                    false
                }
            }
            LiveEvent::Order { symbol, order } if symbol == &self.symbol => {
                if order.active() {
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

    pub fn decide(&mut self) -> Vec<StrategyCommand> {
        let context = MarketContext {
            timestamp: self.timestamp,
            depth: &self.depth,
            position: self.position,
            orders: &self.orders,
            last_trades: &self.last_trades,
        };
        self.strategy.on_event(&context)
    }

    pub fn depth(&self) -> &HashMapMarketDepth {
        &self.depth
    }

    pub fn position(&self) -> f64 {
        self.position
    }
}

/// Safe default strategy for runtime smoke testing. It never creates an order.
pub struct NoopStrategy;

impl Strategy<HashMapMarketDepth> for NoopStrategy {
    fn on_event(
        &mut self,
        _context: &MarketContext<'_, HashMapMarketDepth>,
    ) -> Vec<StrategyCommand> {
        Vec::new()
    }
}
