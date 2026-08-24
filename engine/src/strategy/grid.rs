use std::collections::{HashMap, HashSet};

use crate::{
    depth::{INVALID_MAX, INVALID_MIN, MarketDepth},
    strategy::{MarketContext, Strategy, StrategyCommand},
    types::{OrdType, Side, TimeInForce},
};

#[derive(Clone, Debug)]
pub struct GridConfig {
    pub relative_half_spread: f64,
    pub relative_grid_interval: f64,
    pub grid_num: usize,
    pub min_grid_step: f64,
    pub skew: f64,
    pub order_qty: f64,
    pub max_position: f64,
}

impl GridConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.relative_half_spread.is_finite() || self.relative_half_spread < 0.0 {
            return Err("relative_half_spread must be non-negative");
        }
        if !self.relative_grid_interval.is_finite() || self.relative_grid_interval <= 0.0 {
            return Err("relative_grid_interval must be positive");
        }
        if self.grid_num == 0 {
            return Err("grid_num must be greater than zero");
        }
        if !self.min_grid_step.is_finite() || self.min_grid_step <= 0.0 {
            return Err("min_grid_step must be positive");
        }
        if !self.skew.is_finite() {
            return Err("skew must be finite");
        }
        if !self.order_qty.is_finite() || self.order_qty <= 0.0 {
            return Err("order_qty must be positive");
        }
        if !self.max_position.is_finite() || self.max_position <= 0.0 {
            return Err("max_position must be positive");
        }
        Ok(())
    }
}

/// Grid market-making strategy migrated from `master`'s `examples/algo.rs`.
///
/// This module contains only strategy decisions. It does not know whether commands are executed by
/// the backtest exchange simulator or by a live exchange adapter.
pub struct GridStrategy {
    config: GridConfig,
}

impl GridStrategy {
    pub fn new(config: GridConfig) -> Result<Self, &'static str> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &GridConfig {
        &self.config
    }
}

impl<MD: MarketDepth> Strategy<MD> for GridStrategy {
    fn on_event(&mut self, context: &MarketContext<'_, MD>) -> Vec<StrategyCommand> {
        let depth = context.depth;
        if depth.best_bid_tick() == INVALID_MIN || depth.best_ask_tick() == INVALID_MAX {
            return Vec::new();
        }

        let tick_size = depth.tick_size();
        if !tick_size.is_finite() || tick_size <= 0.0 {
            return Vec::new();
        }

        let min_grid_step = (self.config.min_grid_step / tick_size).round() * tick_size;
        if !min_grid_step.is_finite() || min_grid_step <= 0.0 {
            return Vec::new();
        }

        let mid_price = (depth.best_bid() + depth.best_ask()) / 2.0;
        if !mid_price.is_finite() || mid_price <= 0.0 {
            return Vec::new();
        }

        let normalized_position = context.position / self.config.order_qty;
        let relative_bid_depth =
            self.config.relative_half_spread + self.config.skew * normalized_position;
        let relative_ask_depth =
            self.config.relative_half_spread - self.config.skew * normalized_position;

        // Preserve the master strategy's current alpha=0 behavior. Future alpha models should be
        // injected as a separate signal/fair-value component instead of being hard-coded here.
        let forecast_mid_price = mid_price;
        let bid_price = (forecast_mid_price * (1.0 - relative_bid_depth)).min(depth.best_bid());
        let ask_price = (forecast_mid_price * (1.0 + relative_ask_depth)).max(depth.best_ask());

        let grid_interval =
            ((forecast_mid_price * self.config.relative_grid_interval / min_grid_step).round()
                * min_grid_step)
                .max(min_grid_step);
        if !grid_interval.is_finite() || grid_interval <= 0.0 {
            return Vec::new();
        }

        let mut desired: HashMap<u64, (f64, Side)> = HashMap::new();

        if context.position < self.config.max_position && bid_price.is_finite() {
            let mut price = (bid_price / grid_interval).floor() * grid_interval;
            for _ in 0..self.config.grid_num {
                let order_id = (price / tick_size).round() as u64;
                desired.insert(order_id, (price, Side::Buy));
                price -= grid_interval;
            }
        }

        if context.position > -self.config.max_position && ask_price.is_finite() {
            let mut price = (ask_price / grid_interval).ceil() * grid_interval;
            for _ in 0..self.config.grid_num {
                let order_id = (price / tick_size).round() as u64;
                desired.insert(order_id, (price, Side::Sell));
                price += grid_interval;
            }
        }

        let desired_ids: HashSet<u64> = desired.keys().copied().collect();
        let mut commands = Vec::new();

        for order in context.orders.values() {
            if order.cancellable() && !desired_ids.contains(&order.order_id) {
                commands.push(StrategyCommand::Cancel {
                    order_id: order.order_id,
                });
            }
        }

        for (order_id, (price, side)) in desired {
            if !context.orders.contains_key(&order_id) {
                commands.push(StrategyCommand::Submit {
                    order_id,
                    price,
                    qty: self.config.order_qty,
                    side,
                    time_in_force: TimeInForce::GTX,
                    order_type: OrdType::Limit,
                });
            }
        }

        commands
    }
}
