use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    time::{Duration, Instant},
};

use hftbacktest::types::{
    ErrorKind, Event, LOCAL_ASK_DEPTH_EVENT, LOCAL_BID_DEPTH_EVENT, LOCAL_BUY_TRADE_EVENT,
    LOCAL_SELL_TRADE_EVENT, LiveError, LiveEvent, Order, OrderId, Value,
};

const ACTIVE_AGE: Duration = Duration::from_secs(2);
const DISCONNECTED_AGE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Health {
    Waiting,
    Active,
    Stale,
    Disconnected,
    Critical,
}

pub struct AppState {
    symbol: String,
    tick_size: f64,
    lot_size: f64,
    history_capacity: usize,
    bids: BTreeMap<i64, f64>,
    asks: BTreeMap<i64, f64>,
    recent_trades: VecDeque<Event>,
    orders: HashMap<OrderId, Order>,
    events: VecDeque<String>,
    position: Option<f64>,
    balance: Option<f64>,
    num_fills: u64,
    filled_volume: f64,
    fees: f64,
    last_feed_at: Option<Instant>,
    last_feed_latency_ns: Option<i64>,
    last_order_latency_ns: Option<i64>,
    forced_health: Option<Health>,
    paused: bool,
}

impl AppState {
    pub fn new(
        symbol: impl Into<String>,
        tick_size: f64,
        lot_size: f64,
        history_capacity: usize,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            tick_size,
            lot_size,
            history_capacity: history_capacity.max(1),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            recent_trades: VecDeque::new(),
            orders: HashMap::new(),
            events: VecDeque::new(),
            position: None,
            balance: None,
            num_fills: 0,
            filled_volume: 0.0,
            fees: 0.0,
            last_feed_at: None,
            last_feed_latency_ns: None,
            last_order_latency_ns: None,
            forced_health: None,
            paused: false,
        }
    }

    pub fn apply(&mut self, event: LiveEvent) {
        if self.paused {
            return;
        }
        match event {
            LiveEvent::Feed { symbol, event } if symbol == self.symbol => {
                self.note_feed_at(Instant::now());
                self.last_feed_latency_ns = Some(event.local_ts.saturating_sub(event.exch_ts));
                let tick = (event.px / self.tick_size).round() as i64;
                if event.is(LOCAL_BID_DEPTH_EVENT) {
                    update_level(&mut self.bids, tick, event.qty);
                } else if event.is(LOCAL_ASK_DEPTH_EVENT) {
                    update_level(&mut self.asks, tick, event.qty);
                } else if event.is(LOCAL_BUY_TRADE_EVENT) || event.is(LOCAL_SELL_TRADE_EVENT) {
                    push_bounded(&mut self.recent_trades, event, self.history_capacity);
                }
            }
            LiveEvent::Order { symbol, order } if symbol == self.symbol => {
                self.last_order_latency_ns = (order.exch_timestamp > 0)
                    .then(|| order.exch_timestamp.saturating_sub(order.local_timestamp));
                self.push_event(format!(
                    "ORDER {} {:?} {:?} {:.8} x {}",
                    order.order_id,
                    order.side,
                    order.status,
                    order.price(),
                    order.qty
                ));
                self.orders.insert(order.order_id, order);
            }
            LiveEvent::Position {
                symbol,
                qty,
                exch_ts: _,
            } if symbol == self.symbol => {
                self.position = Some(qty);
                self.push_event(format!("POSITION {qty:+}"));
            }
            LiveEvent::Balance {
                symbol,
                balance,
                exch_ts: _,
            } if symbol == self.symbol => {
                self.balance = Some(balance);
                self.push_event(format!("BALANCE {balance:.8}"));
            }
            LiveEvent::Fill {
                symbol,
                trade_id,
                qty,
                price,
                fee,
                exch_ts: _,
            } if symbol == self.symbol => {
                self.num_fills = self.num_fills.saturating_add(1);
                self.filled_volume += qty.abs();
                self.fees += fee;
                self.push_event(format!("FILL {trade_id} {qty:+} @ {price:.8} fee {fee:.8}"));
            }
            LiveEvent::Error(error) => {
                self.forced_health = match error.kind {
                    ErrorKind::CriticalConnectionError => Some(Health::Critical),
                    ErrorKind::ConnectionInterrupted => Some(Health::Disconnected),
                    ErrorKind::OrderError | ErrorKind::Custom(_) => self.forced_health,
                };
                if is_post_only_rejection(&error) {
                    self.push_event(format!("REJECT POST_ONLY: {:?}", error.value));
                } else {
                    self.push_event(format!("ERROR {:?}: {:?}", error.kind, error.value));
                }
            }
            LiveEvent::BatchStart => self.push_event("BATCH START".into()),
            LiveEvent::BatchEnd => self.push_event("BATCH END".into()),
            LiveEvent::Feed { .. }
            | LiveEvent::Order { .. }
            | LiveEvent::Position { .. }
            | LiveEvent::Balance { .. }
            | LiveEvent::Fill { .. } => {}
        }
    }

    pub fn note_feed_at(&mut self, at: Instant) {
        self.last_feed_at = Some(at);
        if self.forced_health != Some(Health::Critical) {
            self.forced_health = None;
        }
    }

    pub fn health_at(&self, now: Instant) -> Health {
        if let Some(health) = self.forced_health {
            return health;
        }
        match self
            .last_feed_at
            .map(|at| now.saturating_duration_since(at))
        {
            None => Health::Waiting,
            Some(age) if age <= ACTIVE_AGE => Health::Active,
            Some(age) if age <= DISCONNECTED_AGE => Health::Stale,
            Some(_) => Health::Disconnected,
        }
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }
    pub fn tick_size(&self) -> f64 {
        self.tick_size
    }
    pub fn lot_size(&self) -> f64 {
        self.lot_size
    }
    pub fn position(&self) -> Option<f64> {
        self.position
    }
    pub fn balance(&self) -> Option<f64> {
        self.balance
    }
    pub fn num_fills(&self) -> u64 {
        self.num_fills
    }
    pub fn filled_volume(&self) -> f64 {
        self.filled_volume
    }
    pub fn fees(&self) -> f64 {
        self.fees
    }
    pub fn paused(&self) -> bool {
        self.paused
    }
    pub fn toggle_paused(&mut self) {
        self.paused = !self.paused;
    }
    pub fn recent_trades(&self) -> &VecDeque<Event> {
        &self.recent_trades
    }
    pub fn orders(&self) -> &HashMap<OrderId, Order> {
        &self.orders
    }
    pub fn events(&self) -> &VecDeque<String> {
        &self.events
    }
    pub fn last_feed_latency_ns(&self) -> Option<i64> {
        self.last_feed_latency_ns
    }
    pub fn last_order_latency_ns(&self) -> Option<i64> {
        self.last_order_latency_ns
    }

    pub fn best_bid(&self) -> Option<(f64, f64)> {
        self.bids
            .iter()
            .next_back()
            .map(|(tick, qty)| (*tick as f64 * self.tick_size, *qty))
    }

    pub fn best_ask(&self) -> Option<(f64, f64)> {
        self.asks
            .iter()
            .next()
            .map(|(tick, qty)| (*tick as f64 * self.tick_size, *qty))
    }

    pub fn bid_levels(&self, count: usize) -> Vec<(f64, f64)> {
        self.bids
            .iter()
            .rev()
            .take(count)
            .map(|(tick, qty)| (*tick as f64 * self.tick_size, *qty))
            .collect()
    }

    pub fn ask_levels(&self, count: usize) -> Vec<(f64, f64)> {
        self.asks
            .iter()
            .take(count)
            .map(|(tick, qty)| (*tick as f64 * self.tick_size, *qty))
            .collect()
    }

    fn push_event(&mut self, event: String) {
        push_bounded(&mut self.events, event, self.history_capacity);
    }
}

fn is_post_only_rejection(error: &LiveError) -> bool {
    error.kind == ErrorKind::OrderError
        && matches!(
            error.value.get_map().and_then(|value| value.get("code")),
            Some(Value::Int(-5022))
        )
}

fn update_level(levels: &mut BTreeMap<i64, f64>, tick: i64, qty: f64) {
    if qty <= 0.0 {
        levels.remove(&tick);
    } else {
        levels.insert(tick, qty);
    }
}

fn push_bounded<T>(items: &mut VecDeque<T>, item: T, capacity: usize) {
    if items.len() == capacity {
        items.pop_front();
    }
    items.push_back(item);
}
