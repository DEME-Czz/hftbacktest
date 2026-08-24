use std::collections::{HashMap, hash_map::Entry};

use super::{ApplySnapshot, INVALID_MAX, INVALID_MIN, MarketDepth};
use crate::{
    backtest::data::Data,
    prelude::{L2MarketDepth, Side},
    types::{BUY_EVENT, DEPTH_SNAPSHOT_EVENT, EXCH_EVENT, Event, LOCAL_EVENT, SELL_EVENT},
};

/// L2 market depth implementation based on hash maps.
pub struct HashMapMarketDepth {
    pub tick_size: f64,
    pub lot_size: f64,
    pub timestamp: i64,
    pub ask_depth: HashMap<i64, f64>,
    pub bid_depth: HashMap<i64, f64>,
    pub best_bid_tick: i64,
    pub best_ask_tick: i64,
    pub low_bid_tick: i64,
    pub high_ask_tick: i64,
}

#[inline(always)]
fn depth_below(depth: &HashMap<i64, f64>, start: i64, end: i64) -> i64 {
    for tick in (end..start).rev() {
        if *depth.get(&tick).unwrap_or(&0.0) > 0.0 {
            return tick;
        }
    }
    INVALID_MIN
}

#[inline(always)]
fn depth_above(depth: &HashMap<i64, f64>, start: i64, end: i64) -> i64 {
    for tick in (start + 1)..(end + 1) {
        if *depth.get(&tick).unwrap_or(&0.0) > 0.0 {
            return tick;
        }
    }
    INVALID_MAX
}

impl HashMapMarketDepth {
    pub fn new(tick_size: f64, lot_size: f64) -> Self {
        Self {
            tick_size,
            lot_size,
            timestamp: 0,
            ask_depth: HashMap::new(),
            bid_depth: HashMap::new(),
            best_bid_tick: INVALID_MIN,
            best_ask_tick: INVALID_MAX,
            low_bid_tick: INVALID_MAX,
            high_ask_tick: INVALID_MIN,
        }
    }
}

impl L2MarketDepth for HashMapMarketDepth {
    fn update_bid_depth(
        &mut self,
        price: f64,
        qty: f64,
        timestamp: i64,
    ) -> (i64, i64, i64, f64, f64, i64) {
        let price_tick = (price / self.tick_size).round() as i64;
        let qty_lot = (qty / self.lot_size).round() as i64;
        let prev_best_bid_tick = self.best_bid_tick;
        let prev_qty;

        match self.bid_depth.entry(price_tick) {
            Entry::Occupied(mut entry) => {
                prev_qty = *entry.get();
                if qty_lot > 0 {
                    *entry.get_mut() = qty;
                } else {
                    entry.remove();
                }
            }
            Entry::Vacant(entry) => {
                prev_qty = 0.0;
                if qty_lot > 0 {
                    entry.insert(qty);
                }
            }
        }

        if qty_lot == 0 {
            if price_tick == self.best_bid_tick {
                self.best_bid_tick =
                    depth_below(&self.bid_depth, self.best_bid_tick, self.low_bid_tick);
                if self.best_bid_tick == INVALID_MIN {
                    self.low_bid_tick = INVALID_MAX;
                }
            }
        } else {
            if price_tick > self.best_bid_tick {
                self.best_bid_tick = price_tick;
                if self.best_bid_tick >= self.best_ask_tick {
                    self.best_ask_tick =
                        depth_above(&self.ask_depth, self.best_bid_tick, self.high_ask_tick);
                }
            }
            self.low_bid_tick = self.low_bid_tick.min(price_tick);
        }

        self.timestamp = timestamp;
        (
            price_tick,
            prev_best_bid_tick,
            self.best_bid_tick,
            prev_qty,
            qty,
            timestamp,
        )
    }

    fn update_ask_depth(
        &mut self,
        price: f64,
        qty: f64,
        timestamp: i64,
    ) -> (i64, i64, i64, f64, f64, i64) {
        let price_tick = (price / self.tick_size).round() as i64;
        let qty_lot = (qty / self.lot_size).round() as i64;
        let prev_best_ask_tick = self.best_ask_tick;
        let prev_qty;

        match self.ask_depth.entry(price_tick) {
            Entry::Occupied(mut entry) => {
                prev_qty = *entry.get();
                if qty_lot > 0 {
                    *entry.get_mut() = qty;
                } else {
                    entry.remove();
                }
            }
            Entry::Vacant(entry) => {
                prev_qty = 0.0;
                if qty_lot > 0 {
                    entry.insert(qty);
                }
            }
        }

        if qty_lot == 0 {
            if price_tick == self.best_ask_tick {
                self.best_ask_tick =
                    depth_above(&self.ask_depth, self.best_ask_tick, self.high_ask_tick);
                if self.best_ask_tick == INVALID_MAX {
                    self.high_ask_tick = INVALID_MIN;
                }
            }
        } else {
            if price_tick < self.best_ask_tick {
                self.best_ask_tick = price_tick;
                if self.best_bid_tick >= self.best_ask_tick {
                    self.best_bid_tick =
                        depth_below(&self.bid_depth, self.best_ask_tick, self.low_bid_tick);
                }
            }
            self.high_ask_tick = self.high_ask_tick.max(price_tick);
        }

        self.timestamp = timestamp;
        (
            price_tick,
            prev_best_ask_tick,
            self.best_ask_tick,
            prev_qty,
            qty,
            timestamp,
        )
    }

    fn clear_depth(&mut self, side: Side, clear_upto_price: f64) {
        match side {
            Side::Buy => {
                if clear_upto_price.is_finite() {
                    let clear_upto = (clear_upto_price / self.tick_size).round() as i64;
                    if self.best_bid_tick != INVALID_MIN {
                        for tick in clear_upto..=self.best_bid_tick {
                            self.bid_depth.remove(&tick);
                        }
                    }
                    self.best_bid_tick =
                        depth_below(&self.bid_depth, clear_upto - 1, self.low_bid_tick);
                } else {
                    self.bid_depth.clear();
                    self.best_bid_tick = INVALID_MIN;
                }
                if self.best_bid_tick == INVALID_MIN {
                    self.low_bid_tick = INVALID_MAX;
                }
            }
            Side::Sell => {
                if clear_upto_price.is_finite() {
                    let clear_upto = (clear_upto_price / self.tick_size).round() as i64;
                    if self.best_ask_tick != INVALID_MAX {
                        for tick in self.best_ask_tick..=clear_upto {
                            self.ask_depth.remove(&tick);
                        }
                    }
                    self.best_ask_tick =
                        depth_above(&self.ask_depth, clear_upto + 1, self.high_ask_tick);
                } else {
                    self.ask_depth.clear();
                    self.best_ask_tick = INVALID_MAX;
                }
                if self.best_ask_tick == INVALID_MAX {
                    self.high_ask_tick = INVALID_MIN;
                }
            }
            Side::None => {
                self.bid_depth.clear();
                self.ask_depth.clear();
                self.best_bid_tick = INVALID_MIN;
                self.best_ask_tick = INVALID_MAX;
                self.low_bid_tick = INVALID_MAX;
                self.high_ask_tick = INVALID_MIN;
            }
            Side::Unsupported => unreachable!(),
        }
    }
}

impl MarketDepth for HashMapMarketDepth {
    #[inline(always)]
    fn best_bid(&self) -> f64 {
        if self.best_bid_tick == INVALID_MIN {
            f64::NAN
        } else {
            self.best_bid_tick as f64 * self.tick_size
        }
    }

    #[inline(always)]
    fn best_ask(&self) -> f64 {
        if self.best_ask_tick == INVALID_MAX {
            f64::NAN
        } else {
            self.best_ask_tick as f64 * self.tick_size
        }
    }

    #[inline(always)]
    fn best_bid_tick(&self) -> i64 {
        self.best_bid_tick
    }

    #[inline(always)]
    fn best_ask_tick(&self) -> i64 {
        self.best_ask_tick
    }

    #[inline(always)]
    fn best_bid_qty(&self) -> f64 {
        *self.bid_depth.get(&self.best_bid_tick).unwrap_or(&0.0)
    }

    #[inline(always)]
    fn best_ask_qty(&self) -> f64 {
        *self.ask_depth.get(&self.best_ask_tick).unwrap_or(&0.0)
    }

    #[inline(always)]
    fn tick_size(&self) -> f64 {
        self.tick_size
    }

    #[inline(always)]
    fn lot_size(&self) -> f64 {
        self.lot_size
    }

    #[inline(always)]
    fn bid_qty_at_tick(&self, price_tick: i64) -> f64 {
        *self.bid_depth.get(&price_tick).unwrap_or(&0.0)
    }

    #[inline(always)]
    fn ask_qty_at_tick(&self, price_tick: i64) -> f64 {
        *self.ask_depth.get(&price_tick).unwrap_or(&0.0)
    }
}

impl ApplySnapshot for HashMapMarketDepth {
    fn apply_snapshot(&mut self, data: &Data<Event>) {
        self.best_bid_tick = INVALID_MIN;
        self.best_ask_tick = INVALID_MAX;
        self.low_bid_tick = INVALID_MAX;
        self.high_ask_tick = INVALID_MIN;
        self.bid_depth.clear();
        self.ask_depth.clear();

        for row_num in 0..data.len() {
            let event = &data[row_num];
            let price_tick = (event.px / self.tick_size).round() as i64;
            if event.ev & BUY_EVENT == BUY_EVENT {
                self.best_bid_tick = self.best_bid_tick.max(price_tick);
                self.low_bid_tick = self.low_bid_tick.min(price_tick);
                self.bid_depth.insert(price_tick, event.qty);
            } else if event.ev & SELL_EVENT == SELL_EVENT {
                self.best_ask_tick = self.best_ask_tick.min(price_tick);
                self.high_ask_tick = self.high_ask_tick.max(price_tick);
                self.ask_depth.insert(price_tick, event.qty);
            }
        }
    }

    fn snapshot(&self) -> Vec<Event> {
        let mut events = Vec::with_capacity(self.bid_depth.len() + self.ask_depth.len());

        let mut bids = self
            .bid_depth
            .iter()
            .filter(|(tick, _)| **tick <= self.best_bid_tick)
            .map(|(tick, qty)| (*tick, *qty))
            .collect::<Vec<_>>();
        bids.sort_by_key(|bid| std::cmp::Reverse(bid.0));
        for (price_tick, qty) in bids {
            events.push(Event {
                ev: EXCH_EVENT | LOCAL_EVENT | BUY_EVENT | DEPTH_SNAPSHOT_EVENT,
                exch_ts: 0,
                local_ts: 0,
                px: price_tick as f64 * self.tick_size,
                qty,
                order_id: 0,
                ival: 0,
                fval: 0.0,
            });
        }

        let mut asks = self
            .ask_depth
            .iter()
            .filter(|(tick, _)| **tick >= self.best_ask_tick)
            .map(|(tick, qty)| (*tick, *qty))
            .collect::<Vec<_>>();
        asks.sort_by_key(|ask| ask.0);
        for (price_tick, qty) in asks {
            events.push(Event {
                ev: EXCH_EVENT | LOCAL_EVENT | SELL_EVENT | DEPTH_SNAPSHOT_EVENT,
                exch_ts: 0,
                local_ts: 0,
                px: price_tick as f64 * self.tick_size,
                qty,
                order_id: 0,
                ival: 0,
                fval: 0.0,
            });
        }

        events
    }
}
