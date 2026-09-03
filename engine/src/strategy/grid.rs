use std::collections::HashSet;

use crate::{
    depth::{INVALID_MAX, INVALID_MIN, MarketDepth},
    strategy::{MarketContext, Strategy, StrategyCommand},
    types::{OrdType, Order, Side, TimeInForce},
};

const NANOS_PER_MILLISECOND: i64 = 1_000_000;
const DEFENSIVE_QTY_MULTIPLIER: f64 = 0.5;

#[derive(Clone, Debug)]
pub struct GridConfig {
    pub relative_half_spread: f64,
    pub relative_grid_interval: f64,
    pub grid_num: usize,
    pub min_grid_step: f64,
    /// Maximum relative reservation-price shift when inventory reaches +/- max_position.
    pub skew: f64,
    pub order_qty: f64,
    pub max_position: f64,
    pub inventory_reduce_threshold: f64,
    pub inventory_stop_threshold: f64,
    pub requote_ticks: u64,
    pub min_quote_lifetime_ms: u64,
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
        if !self.skew.is_finite() || self.skew < 0.0 || self.skew > 1.0 {
            return Err("skew must be within [0, 1]");
        }
        if !self.order_qty.is_finite() || self.order_qty <= 0.0 {
            return Err("order_qty must be positive");
        }
        if !self.max_position.is_finite() || self.max_position <= 0.0 {
            return Err("max_position must be positive");
        }
        if !self.inventory_reduce_threshold.is_finite()
            || !self.inventory_stop_threshold.is_finite()
            || self.inventory_reduce_threshold <= 0.0
            || self.inventory_reduce_threshold >= self.inventory_stop_threshold
            || self.inventory_stop_threshold > 1.0
        {
            return Err("inventory thresholds must satisfy 0 < reduce < stop <= 1");
        }
        if self.requote_ticks == 0 {
            return Err("requote_ticks must be greater than zero");
        }
        if self.min_quote_lifetime_ms == 0 {
            return Err("min_quote_lifetime_ms must be greater than zero");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct DesiredQuote {
    order_id: u64,
    price_tick: i64,
    price: f64,
    qty: f64,
    side: Side,
    matched: bool,
}

/// Exchange-independent double-sided grid market-making strategy.
///
/// Inventory is normalized by `max_position`, not by order size. This keeps reservation-price skew
/// stable when `order_qty` changes. Once inventory enters the defensive zone the strategy reduces
/// risk-increasing quote size; once it enters the stop zone it removes that side entirely and keeps
/// only quotes that can reduce inventory. Stop-zone reducing quotes are capped by the actual
/// position so a complete fill cannot cross through flat into a new opposite position.
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

    fn inventory_ratio(&self, position: f64) -> f64 {
        (position / self.config.max_position).clamp(-1.0, 1.0)
    }

    fn quote_quantities(&self, inventory_ratio: f64) -> (f64, f64) {
        let inventory_abs = inventory_ratio.abs();
        let risk_increasing_multiplier = if inventory_abs >= self.config.inventory_stop_threshold {
            0.0
        } else if inventory_abs >= self.config.inventory_reduce_threshold {
            DEFENSIVE_QTY_MULTIPLIER
        } else {
            1.0
        };

        if inventory_ratio > 0.0 {
            (
                self.config.order_qty * risk_increasing_multiplier,
                self.config.order_qty,
            )
        } else if inventory_ratio < 0.0 {
            (
                self.config.order_qty,
                self.config.order_qty * risk_increasing_multiplier,
            )
        } else {
            (self.config.order_qty, self.config.order_qty)
        }
    }

    fn quote_old_enough(&self, context_timestamp: i64, order: &Order) -> bool {
        if order.local_timestamp <= 0 || context_timestamp <= 0 {
            return true;
        }
        let min_lifetime_ns = i64::try_from(self.config.min_quote_lifetime_ms)
            .unwrap_or(i64::MAX)
            .saturating_mul(NANOS_PER_MILLISECOND);
        context_timestamp.saturating_sub(order.local_timestamp) >= min_lifetime_ns
    }

    fn desired_quotes<MD: MarketDepth>(
        &self,
        context: &MarketContext<'_, MD>,
    ) -> Vec<DesiredQuote> {
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

        let inventory_ratio = self.inventory_ratio(context.position);
        let stop_zone = inventory_ratio.abs() >= self.config.inventory_stop_threshold;
        let reservation_price = mid_price * (1.0 - self.config.skew * inventory_ratio);
        if !reservation_price.is_finite() || reservation_price <= 0.0 {
            return Vec::new();
        }

        let bid_price =
            (reservation_price * (1.0 - self.config.relative_half_spread)).min(depth.best_bid());
        let ask_price =
            (reservation_price * (1.0 + self.config.relative_half_spread)).max(depth.best_ask());
        let grid_interval = ((mid_price * self.config.relative_grid_interval / min_grid_step)
            .round()
            * min_grid_step)
            .max(min_grid_step);
        if !grid_interval.is_finite() || grid_interval <= 0.0 {
            return Vec::new();
        }

        let (bid_qty, ask_qty) = self.quote_quantities(inventory_ratio);
        let mut desired = Vec::with_capacity(self.config.grid_num.saturating_mul(2));

        if context.position < self.config.max_position && bid_qty > 0.0 && bid_price.is_finite() {
            let mut price = (bid_price / grid_interval).floor() * grid_interval;
            let mut remaining_reduce_qty = if stop_zone && context.position < 0.0 {
                -context.position
            } else {
                f64::INFINITY
            };
            for _ in 0..self.config.grid_num {
                let qty = bid_qty.min(remaining_reduce_qty);
                if !qty.is_finite() && remaining_reduce_qty.is_finite() || qty <= 0.0 {
                    break;
                }
                let qty = if qty.is_finite() { qty } else { bid_qty };
                let price_tick = (price / tick_size).round() as i64;
                if price_tick > 0 {
                    desired.push(DesiredQuote {
                        order_id: price_tick as u64,
                        price_tick,
                        price,
                        qty,
                        side: Side::Buy,
                        matched: false,
                    });
                }
                if remaining_reduce_qty.is_finite() {
                    remaining_reduce_qty = (remaining_reduce_qty - qty).max(0.0);
                }
                price -= grid_interval;
            }
        }

        if context.position > -self.config.max_position && ask_qty > 0.0 && ask_price.is_finite() {
            let mut price = (ask_price / grid_interval).ceil() * grid_interval;
            let mut remaining_reduce_qty = if stop_zone && context.position > 0.0 {
                context.position
            } else {
                f64::INFINITY
            };
            for _ in 0..self.config.grid_num {
                let qty = ask_qty.min(remaining_reduce_qty);
                if !qty.is_finite() && remaining_reduce_qty.is_finite() || qty <= 0.0 {
                    break;
                }
                let qty = if qty.is_finite() { qty } else { ask_qty };
                let price_tick = (price / tick_size).round() as i64;
                if price_tick > 0 {
                    desired.push(DesiredQuote {
                        order_id: price_tick as u64,
                        price_tick,
                        price,
                        qty,
                        side: Side::Sell,
                        matched: false,
                    });
                }
                if remaining_reduce_qty.is_finite() {
                    remaining_reduce_qty = (remaining_reduce_qty - qty).max(0.0);
                }
                price += grid_interval;
            }
        }

        desired
    }
}

impl<MD: MarketDepth> Strategy<MD> for GridStrategy {
    fn on_event(&mut self, context: &MarketContext<'_, MD>) -> Vec<StrategyCommand> {
        let mut desired = self.desired_quotes(context);
        if desired.is_empty() {
            return Vec::new();
        }

        let emergency_inventory = self.inventory_ratio(context.position).abs()
            >= self.config.inventory_stop_threshold;
        let mut commands = Vec::new();
        let mut canceled_ids = HashSet::new();

        for order in context.orders.values() {
            let nearest = desired
                .iter()
                .enumerate()
                .filter(|(_, quote)| !quote.matched && quote.side == order.side)
                .min_by_key(|(_, quote)| order.price_tick.abs_diff(quote.price_tick))
                .map(|(index, _)| index);

            let Some(index) = nearest else {
                if order.cancellable()
                    && (emergency_inventory || self.quote_old_enough(context.timestamp, order))
                {
                    commands.push(StrategyCommand::Cancel {
                        order_id: order.order_id,
                    });
                    canceled_ids.insert(order.order_id);
                }
                continue;
            };

            let quote = &desired[index];
            let price_distance = order.price_tick.abs_diff(quote.price_tick);
            let qty_tolerance = order.qty.abs().max(quote.qty.abs()).max(1.0) * f64::EPSILON * 8.0;
            let qty_matches = (order.qty - quote.qty).abs() <= qty_tolerance;
            let keep_for_hysteresis = price_distance <= self.config.requote_ticks && qty_matches;
            let old_enough =
                emergency_inventory || self.quote_old_enough(context.timestamp, order);

            if keep_for_hysteresis || !old_enough || !order.cancellable() {
                desired[index].matched = true;
                continue;
            }

            // Cancel first and wait for the exchange terminal update before submitting the
            // replacement. This avoids cancel+submit bursts and preserves a bounded order count.
            commands.push(StrategyCommand::Cancel {
                order_id: order.order_id,
            });
            canceled_ids.insert(order.order_id);
            desired[index].matched = true;
        }

        for quote in desired.into_iter().filter(|quote| !quote.matched) {
            if canceled_ids.contains(&quote.order_id)
                || context.orders.contains_key(&quote.order_id)
            {
                continue;
            }
            commands.push(StrategyCommand::Submit {
                order_id: quote.order_id,
                price: quote.price,
                qty: quote.qty,
                side: quote.side,
                time_in_force: TimeInForce::GTX,
                order_type: OrdType::Limit,
            });
        }

        commands
    }
}
