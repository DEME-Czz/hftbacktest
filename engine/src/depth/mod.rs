use std::collections::HashMap;

pub use hashmapmarketdepth::HashMapMarketDepth;
pub use roivectormarketdepth::ROIVectorMarketDepth;

use crate::prelude::Side;

mod hashmapmarketdepth;
mod roivectormarketdepth;

use crate::{
    backtest::data::Data,
    types::{Event, OrderId},
};

pub const INVALID_MIN: i64 = i64::MIN;
pub const INVALID_MAX: i64 = i64::MAX;

pub trait MarketDepth {
    fn best_bid(&self) -> f64;
    fn best_ask(&self) -> f64;
    fn best_bid_tick(&self) -> i64;
    fn best_ask_tick(&self) -> i64;
    fn best_bid_qty(&self) -> f64;
    fn best_ask_qty(&self) -> f64;
    fn tick_size(&self) -> f64;
    fn lot_size(&self) -> f64;
    fn bid_qty_at_tick(&self, price_tick: i64) -> f64;
    fn ask_qty_at_tick(&self, price_tick: i64) -> f64;
}

pub trait L2MarketDepth {
    fn update_bid_depth(
        &mut self,
        price: f64,
        qty: f64,
        timestamp: i64,
    ) -> (i64, i64, i64, f64, f64, i64);

    fn update_ask_depth(
        &mut self,
        price: f64,
        qty: f64,
        timestamp: i64,
    ) -> (i64, i64, i64, f64, f64, i64);

    fn clear_depth(&mut self, side: Side, clear_upto_price: f64);
}

pub trait ApplySnapshot {
    fn apply_snapshot(&mut self, data: &Data<Event>);
    fn snapshot(&self) -> Vec<Event>;
}

#[derive(Debug)]
pub struct L3Order {
    pub order_id: OrderId,
    pub side: Side,
    pub price_tick: i64,
    pub qty: f64,
    pub timestamp: i64,
}

pub trait L3MarketDepth: MarketDepth {
    type Error;

    fn add_buy_order(
        &mut self,
        order_id: OrderId,
        px: f64,
        qty: f64,
        timestamp: i64,
    ) -> Result<(i64, i64), Self::Error>;

    fn add_sell_order(
        &mut self,
        order_id: OrderId,
        px: f64,
        qty: f64,
        timestamp: i64,
    ) -> Result<(i64, i64), Self::Error>;

    fn delete_order(
        &mut self,
        order_id: OrderId,
        timestamp: i64,
    ) -> Result<(Side, i64, i64), Self::Error>;

    fn modify_order(
        &mut self,
        order_id: OrderId,
        px: f64,
        qty: f64,
        timestamp: i64,
    ) -> Result<(Side, i64, i64), Self::Error>;

    fn clear_orders(&mut self, side: Side);
    fn orders(&self) -> &HashMap<OrderId, L3Order>;
}

pub trait L1MarketDepth {
    fn update_best_bid(
        &mut self,
        px: f64,
        qty: f64,
        timestamp: i64,
    ) -> (i64, i64, i64, f64, f64, i64);

    fn update_best_ask(
        &mut self,
        px: f64,
        qty: f64,
        timestamp: i64,
    ) -> (i64, i64, i64, f64, f64, i64);
}
