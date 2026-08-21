use std::collections::HashMap;

use hftbacktest::{
    depth::{HashMapMarketDepth, L2MarketDepth, MarketDepth},
    strategy::{
        BuiltinStrategy, BuiltinStrategyConfig, GridConfig, MarketContext, Strategy,
        StrategyCommand,
    },
    types::{
        BUY_EVENT, DEPTH_CLEAR_EVENT, DEPTH_EVENT, Event, LiveEvent, OrdType, Order, OrderId, SELL_EVENT,
        Side, Status, TRADE_EVENT, TimeInForce,
    },
};
use serde::Deserialize;

use crate::risk::RiskConfig;

#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub strategies: Vec<LiveStrategyConfig>,
    #[serde(default)]
    pub risk: RiskConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LiveStrategyConfig {
    pub symbol: String,
    pub tick_size: f64,
    pub lot_size: f64,
    #[serde(flatten)]
    pub strategy: StrategyConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrategyConfig {
    Grid {
        relative_half_spread: f64,
        relative_grid_interval: f64,
        grid_num: usize,
        min_grid_step: f64,
        skew: f64,
        order_qty: f64,
        max_position: f64,
    },
}

impl LiveStrategyConfig {
    pub fn build_strategy(&self) -> Result<BuiltinStrategy, &'static str> {
        let config = match &self.strategy {
            StrategyConfig::Grid {
                relative_half_spread,
                relative_grid_interval,
                grid_num,
                min_grid_step,
                skew,
                order_qty,
                max_position,
            } => BuiltinStrategyConfig::Grid(GridConfig {
                relative_half_spread: *relative_half_spread,
                relative_grid_interval: *relative_grid_interval,
                grid_num: *grid_num,
                min_grid_step: *min_grid_step,
                skew: *skew,
                order_qty: *order_qty,
                max_position: *max_position,
            }),
        };
        BuiltinStrategy::from_config(config)
    }
}

/// In-process normalized market/account state used by live strategies.
///
/// The runtime owns only normalized state. Exchange transport stays in the connector and strategy
/// decisions stay in the engine strategy layer.
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

    pub fn symbol(&self) -> &str { &self.symbol }

    /// Applies a normalized live event. Depth updates only mutate state; decision should happen at
    /// the matching `PublishEvent::BatchEnd(symbol)` so the strategy observes one complete Binance
    /// depth batch.
    pub fn apply(&mut self, live: &LiveEvent) -> bool {
        match live {
            LiveEvent::Feed { symbol, event } if symbol == &self.symbol => {
                self.timestamp = event.local_ts;
                if event.is(DEPTH_EVENT | BUY_EVENT) {
                    self.depth.update_bid_depth(event.px, event.qty, event.local_ts);
                    false
                } else if event.is(DEPTH_EVENT | SELL_EVENT) {
                    self.depth.update_ask_depth(event.px, event.qty, event.local_ts);
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
                if order.active() || order.pending() {
                    self.orders.insert(order.order_id, order.clone());
                } else {
                    self.orders.remove(&order.order_id);
                }
                true
            }
            LiveEvent::Position { symbol, qty, exch_ts } if symbol == &self.symbol => {
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

    /// Stages a local pending submit before the asynchronous REST request is started. This prevents
    /// the next 100ms depth batch from generating the same order again while REST/WS is in flight.
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

    pub fn stage_cancel(&mut self, order_id: OrderId) -> Option<Order> {
        let order = self.orders.get_mut(&order_id)?;
        if !order.cancellable() {
            return None;
        }
        order.req = Status::Canceled;
        Some(order.clone())
    }

    pub fn depth(&self) -> &HashMapMarketDepth { &self.depth }
    pub fn position(&self) -> f64 { self.position }
    pub fn open_orders(&self) -> usize { self.orders.len() }
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
