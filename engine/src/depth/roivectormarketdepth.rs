use super::{ApplySnapshot, INVALID_MAX, INVALID_MIN, MarketDepth};
use crate::{
    backtest::data::Data,
    prelude::{L2MarketDepth, Side},
    types::{BUY_EVENT, Event, SELL_EVENT},
};

/// L2 market depth backed by vectors inside a configured range of interest.
pub struct ROIVectorMarketDepth {
    pub tick_size: f64,
    pub lot_size: f64,
    pub timestamp: i64,
    pub ask_depth: Vec<f64>,
    pub bid_depth: Vec<f64>,
    pub best_bid_tick: i64,
    pub best_ask_tick: i64,
    pub low_bid_tick: i64,
    pub high_ask_tick: i64,
    pub roi_ub: i64,
    pub roi_lb: i64,
}

#[inline(always)]
fn depth_below(depth: &[f64], start: i64, end: i64, roi_lb: i64, roi_ub: i64) -> i64 {
    let start = (start.min(roi_ub) - roi_lb).max(0) as usize;
    let end = (end.max(roi_lb) - roi_lb).max(0) as usize;
    for index in (end..start).rev() {
        if unsafe { *depth.get_unchecked(index) } > 0.0 {
            return index as i64 + roi_lb;
        }
    }
    INVALID_MIN
}

#[inline(always)]
fn depth_above(depth: &[f64], start: i64, end: i64, roi_lb: i64, roi_ub: i64) -> i64 {
    let start = (start.max(roi_lb) - roi_lb).max(0) as usize;
    let end = (end.min(roi_ub) - roi_lb).max(0) as usize;
    if start >= end || start + 1 >= depth.len() {
        return INVALID_MAX;
    }
    for index in (start + 1)..=(end.min(depth.len() - 1)) {
        if unsafe { *depth.get_unchecked(index) } > 0.0 {
            return index as i64 + roi_lb;
        }
    }
    INVALID_MAX
}

impl ROIVectorMarketDepth {
    pub fn new(tick_size: f64, lot_size: f64, roi_lb: f64, roi_ub: f64) -> Self {
        let roi_lb = (roi_lb / tick_size).round() as i64;
        let roi_ub = (roi_ub / tick_size).round() as i64;
        assert!(
            roi_lb <= roi_ub,
            "ROI lower bound must not exceed upper bound"
        );
        let roi_range = (roi_ub + 1 - roi_lb) as usize;
        Self {
            tick_size,
            lot_size,
            timestamp: 0,
            ask_depth: vec![0.0; roi_range],
            bid_depth: vec![0.0; roi_range],
            best_bid_tick: INVALID_MIN,
            best_ask_tick: INVALID_MAX,
            low_bid_tick: INVALID_MAX,
            high_ask_tick: INVALID_MIN,
            roi_lb,
            roi_ub,
        }
    }

    pub fn bid_depth(&self) -> &[f64] {
        &self.bid_depth
    }

    pub fn ask_depth(&self) -> &[f64] {
        &self.ask_depth
    }

    pub fn roi(&self) -> (f64, f64) {
        (
            self.roi_lb as f64 * self.tick_size,
            self.roi_ub as f64 * self.tick_size,
        )
    }

    pub fn roi_tick(&self) -> (i64, i64) {
        (self.roi_lb, self.roi_ub)
    }

    #[inline(always)]
    fn index(&self, price_tick: i64) -> Option<usize> {
        if price_tick < self.roi_lb || price_tick > self.roi_ub {
            None
        } else {
            Some((price_tick - self.roi_lb) as usize)
        }
    }
}

impl L2MarketDepth for ROIVectorMarketDepth {
    fn update_bid_depth(
        &mut self,
        price: f64,
        qty: f64,
        timestamp: i64,
    ) -> (i64, i64, i64, f64, f64, i64) {
        let price_tick = (price / self.tick_size).round() as i64;
        let qty_lot = (qty / self.lot_size).round() as i64;
        let prev_best_bid_tick = self.best_bid_tick;
        let Some(index) = self.index(price_tick) else {
            return (
                price_tick,
                prev_best_bid_tick,
                self.best_bid_tick,
                0.0,
                qty,
                timestamp,
            );
        };
        let prev_qty = self.bid_depth[index];
        self.bid_depth[index] = qty;

        if qty_lot == 0 {
            if price_tick == self.best_bid_tick {
                self.best_bid_tick = depth_below(
                    &self.bid_depth,
                    self.best_bid_tick,
                    if self.low_bid_tick == INVALID_MAX {
                        self.roi_lb
                    } else {
                        self.low_bid_tick
                    },
                    self.roi_lb,
                    self.roi_ub,
                );
                if self.best_bid_tick == INVALID_MIN {
                    self.low_bid_tick = INVALID_MAX;
                }
            }
        } else {
            if price_tick > self.best_bid_tick {
                self.best_bid_tick = price_tick;
                if self.best_bid_tick >= self.best_ask_tick {
                    self.best_ask_tick = depth_above(
                        &self.ask_depth,
                        self.best_bid_tick,
                        if self.high_ask_tick == INVALID_MIN {
                            self.roi_ub
                        } else {
                            self.high_ask_tick
                        },
                        self.roi_lb,
                        self.roi_ub,
                    );
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
        let Some(index) = self.index(price_tick) else {
            return (
                price_tick,
                prev_best_ask_tick,
                self.best_ask_tick,
                0.0,
                qty,
                timestamp,
            );
        };
        let prev_qty = self.ask_depth[index];
        self.ask_depth[index] = qty;

        if qty_lot == 0 {
            if price_tick == self.best_ask_tick {
                self.best_ask_tick = depth_above(
                    &self.ask_depth,
                    self.best_ask_tick,
                    if self.high_ask_tick == INVALID_MIN {
                        self.roi_ub
                    } else {
                        self.high_ask_tick
                    },
                    self.roi_lb,
                    self.roi_ub,
                );
                if self.best_ask_tick == INVALID_MAX {
                    self.high_ask_tick = INVALID_MIN;
                }
            }
        } else {
            if price_tick < self.best_ask_tick {
                self.best_ask_tick = price_tick;
                if self.best_bid_tick >= self.best_ask_tick {
                    self.best_bid_tick = depth_below(
                        &self.bid_depth,
                        self.best_ask_tick,
                        if self.low_bid_tick == INVALID_MAX {
                            self.roi_lb
                        } else {
                            self.low_bid_tick
                        },
                        self.roi_lb,
                        self.roi_ub,
                    );
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
                        let from = clear_upto.max(self.roi_lb);
                        let to = self.best_bid_tick.min(self.roi_ub);
                        if from <= to {
                            for tick in from..=to {
                                self.bid_depth[(tick - self.roi_lb) as usize] = 0.0;
                            }
                        }
                    }
                    self.best_bid_tick = depth_below(
                        &self.bid_depth,
                        (clear_upto - 1).clamp(self.roi_lb, self.roi_ub),
                        if self.low_bid_tick == INVALID_MAX {
                            self.roi_lb
                        } else {
                            self.low_bid_tick
                        },
                        self.roi_lb,
                        self.roi_ub,
                    );
                } else {
                    self.bid_depth.fill(0.0);
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
                        let from = self.best_ask_tick.max(self.roi_lb);
                        let to = clear_upto.min(self.roi_ub);
                        if from <= to {
                            for tick in from..=to {
                                self.ask_depth[(tick - self.roi_lb) as usize] = 0.0;
                            }
                        }
                    }
                    self.best_ask_tick = depth_above(
                        &self.ask_depth,
                        (clear_upto + 1).clamp(self.roi_lb, self.roi_ub),
                        if self.high_ask_tick == INVALID_MIN {
                            self.roi_ub
                        } else {
                            self.high_ask_tick
                        },
                        self.roi_lb,
                        self.roi_ub,
                    );
                } else {
                    self.ask_depth.fill(0.0);
                    self.best_ask_tick = INVALID_MAX;
                }
                if self.best_ask_tick == INVALID_MAX {
                    self.high_ask_tick = INVALID_MIN;
                }
            }
            Side::None => {
                self.bid_depth.fill(0.0);
                self.ask_depth.fill(0.0);
                self.best_bid_tick = INVALID_MIN;
                self.best_ask_tick = INVALID_MAX;
                self.low_bid_tick = INVALID_MAX;
                self.high_ask_tick = INVALID_MIN;
            }
            Side::Unsupported => unreachable!(),
        }
    }
}

impl MarketDepth for ROIVectorMarketDepth {
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
        self.index(self.best_bid_tick)
            .map(|index| self.bid_depth[index])
            .unwrap_or(0.0)
    }

    #[inline(always)]
    fn best_ask_qty(&self) -> f64 {
        self.index(self.best_ask_tick)
            .map(|index| self.ask_depth[index])
            .unwrap_or(0.0)
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
        self.index(price_tick)
            .map(|index| self.bid_depth[index])
            .unwrap_or(0.0)
    }

    #[inline(always)]
    fn ask_qty_at_tick(&self, price_tick: i64) -> f64 {
        self.index(price_tick)
            .map(|index| self.ask_depth[index])
            .unwrap_or(0.0)
    }
}

impl ApplySnapshot for ROIVectorMarketDepth {
    fn apply_snapshot(&mut self, data: &Data<Event>) {
        self.best_bid_tick = INVALID_MIN;
        self.best_ask_tick = INVALID_MAX;
        self.low_bid_tick = INVALID_MAX;
        self.high_ask_tick = INVALID_MIN;
        self.bid_depth.fill(0.0);
        self.ask_depth.fill(0.0);

        for row_num in 0..data.len() {
            let event = &data[row_num];
            let price_tick = (event.px / self.tick_size).round() as i64;
            let Some(index) = self.index(price_tick) else {
                continue;
            };
            if event.ev & BUY_EVENT == BUY_EVENT {
                self.best_bid_tick = self.best_bid_tick.max(price_tick);
                self.low_bid_tick = self.low_bid_tick.min(price_tick);
                self.bid_depth[index] = event.qty;
            } else if event.ev & SELL_EVENT == SELL_EVENT {
                self.best_ask_tick = self.best_ask_tick.min(price_tick);
                self.high_ask_tick = self.high_ask_tick.max(price_tick);
                self.ask_depth[index] = event.qty;
            }
        }
    }

    fn snapshot(&self) -> Vec<Event> {
        Vec::new()
    }
}
