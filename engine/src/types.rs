use std::{
    any::Any,
    collections::HashMap,
    fmt::{Debug, Formatter},
};

use bincode::{
    BorrowDecode, Decode, Encode,
    de::{BorrowDecoder, Decoder},
    enc::Encoder,
    error::{DecodeError, EncodeError},
};
use dyn_clone::DynClone;
use thiserror::Error;

use crate::{
    backtest::data::{Field, NpyDTyped, POD},
    depth::MarketDepth,
};

#[derive(Clone, Debug, Decode, Encode)]
pub enum Value {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<Value>),
    Map(HashMap<String, Value>),
    Empty,
}

impl From<anyhow::Error> for Value {
    fn from(value: anyhow::Error) -> Self { Self::String(value.to_string()) }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Decode, Encode)]
pub enum ErrorKind {
    ConnectionInterrupted,
    CriticalConnectionError,
    OrderError,
    Custom(i64),
}

#[derive(Clone, Debug, Decode, Encode)]
pub struct LiveError {
    pub kind: ErrorKind,
    pub value: Value,
}

impl LiveError {
    pub fn new(kind: ErrorKind) -> Self { Self { kind, value: Value::Empty } }
    pub fn with(kind: ErrorKind, value: Value) -> Self { Self { kind, value } }
    pub fn value(&self) -> &Value { &self.value }
}

#[derive(Clone, Debug, Decode, Encode)]
pub enum LiveEvent {
    BatchStart,
    BatchEnd,
    Feed { symbol: String, event: Event },
    Order { symbol: String, order: Order },
    Position { symbol: String, qty: f64, exch_ts: i64 },
    Error(LiveError),
}

pub const BUY_EVENT: u64 = 1 << 29;
pub const SELL_EVENT: u64 = 1 << 28;
pub const DEPTH_EVENT: u64 = 1;
pub const TRADE_EVENT: u64 = 2;
pub const DEPTH_CLEAR_EVENT: u64 = 3;
pub const DEPTH_SNAPSHOT_EVENT: u64 = 4;
pub const DEPTH_BBO_EVENT: u64 = 5;
pub const ADD_ORDER_EVENT: u64 = 10;
pub const CANCEL_ORDER_EVENT: u64 = 11;
pub const MODIFY_ORDER_EVENT: u64 = 12;
pub const FILL_EVENT: u64 = 13;
pub const EXCH_EVENT: u64 = 1 << 31;
pub const LOCAL_EVENT: u64 = 1 << 30;

pub const LOCAL_DEPTH_CLEAR_EVENT: u64 = DEPTH_CLEAR_EVENT | LOCAL_EVENT;
pub const EXCH_DEPTH_CLEAR_EVENT: u64 = DEPTH_CLEAR_EVENT | EXCH_EVENT;
pub const LOCAL_BID_DEPTH_EVENT: u64 = DEPTH_EVENT | BUY_EVENT | LOCAL_EVENT;
pub const LOCAL_ASK_DEPTH_EVENT: u64 = DEPTH_EVENT | SELL_EVENT | LOCAL_EVENT;
pub const LOCAL_BID_DEPTH_CLEAR_EVENT: u64 = DEPTH_CLEAR_EVENT | BUY_EVENT | LOCAL_EVENT;
pub const LOCAL_ASK_DEPTH_CLEAR_EVENT: u64 = DEPTH_CLEAR_EVENT | SELL_EVENT | LOCAL_EVENT;
pub const LOCAL_BID_DEPTH_SNAPSHOT_EVENT: u64 = DEPTH_SNAPSHOT_EVENT | BUY_EVENT | LOCAL_EVENT;
pub const LOCAL_ASK_DEPTH_SNAPSHOT_EVENT: u64 = DEPTH_SNAPSHOT_EVENT | SELL_EVENT | LOCAL_EVENT;
pub const LOCAL_BID_DEPTH_BBO_EVENT: u64 = DEPTH_BBO_EVENT | BUY_EVENT | LOCAL_EVENT;
pub const LOCAL_ASK_DEPTH_BBO_EVENT: u64 = DEPTH_BBO_EVENT | SELL_EVENT | LOCAL_EVENT;
pub const LOCAL_TRADE_EVENT: u64 = TRADE_EVENT | LOCAL_EVENT;
pub const LOCAL_BUY_TRADE_EVENT: u64 = LOCAL_TRADE_EVENT | BUY_EVENT;
pub const LOCAL_SELL_TRADE_EVENT: u64 = LOCAL_TRADE_EVENT | SELL_EVENT;
pub const EXCH_BID_DEPTH_EVENT: u64 = DEPTH_EVENT | BUY_EVENT | EXCH_EVENT;
pub const EXCH_ASK_DEPTH_EVENT: u64 = DEPTH_EVENT | SELL_EVENT | EXCH_EVENT;
pub const EXCH_BID_DEPTH_CLEAR_EVENT: u64 = DEPTH_CLEAR_EVENT | BUY_EVENT | EXCH_EVENT;
pub const EXCH_ASK_DEPTH_CLEAR_EVENT: u64 = DEPTH_CLEAR_EVENT | SELL_EVENT | EXCH_EVENT;
pub const EXCH_BID_DEPTH_SNAPSHOT_EVENT: u64 = DEPTH_SNAPSHOT_EVENT | BUY_EVENT | EXCH_EVENT;
pub const EXCH_ASK_DEPTH_SNAPSHOT_EVENT: u64 = DEPTH_SNAPSHOT_EVENT | SELL_EVENT | EXCH_EVENT;
pub const EXCH_BID_DEPTH_BBO_EVENT: u64 = DEPTH_BBO_EVENT | BUY_EVENT | EXCH_EVENT;
pub const EXCH_ASK_DEPTH_BBO_EVENT: u64 = DEPTH_BBO_EVENT | SELL_EVENT | EXCH_EVENT;
pub const EXCH_TRADE_EVENT: u64 = TRADE_EVENT | EXCH_EVENT;
pub const EXCH_BUY_TRADE_EVENT: u64 = EXCH_TRADE_EVENT | BUY_EVENT;
pub const EXCH_SELL_TRADE_EVENT: u64 = EXCH_TRADE_EVENT | SELL_EVENT;
pub const LOCAL_ADD_ORDER_EVENT: u64 = LOCAL_EVENT | ADD_ORDER_EVENT;
pub const LOCAL_BID_ADD_ORDER_EVENT: u64 = BUY_EVENT | LOCAL_ADD_ORDER_EVENT;
pub const LOCAL_ASK_ADD_ORDER_EVENT: u64 = SELL_EVENT | LOCAL_ADD_ORDER_EVENT;
pub const LOCAL_CANCEL_ORDER_EVENT: u64 = LOCAL_EVENT | CANCEL_ORDER_EVENT;
pub const LOCAL_MODIFY_ORDER_EVENT: u64 = LOCAL_EVENT | MODIFY_ORDER_EVENT;
pub const LOCAL_FILL_EVENT: u64 = LOCAL_EVENT | FILL_EVENT;
pub const EXCH_ADD_ORDER_EVENT: u64 = EXCH_EVENT | ADD_ORDER_EVENT;
pub const EXCH_BID_ADD_ORDER_EVENT: u64 = BUY_EVENT | EXCH_ADD_ORDER_EVENT;
pub const EXCH_ASK_ADD_ORDER_EVENT: u64 = SELL_EVENT | EXCH_ADD_ORDER_EVENT;
pub const EXCH_CANCEL_ORDER_EVENT: u64 = EXCH_EVENT | CANCEL_ORDER_EVENT;
pub const EXCH_MODIFY_ORDER_EVENT: u64 = EXCH_EVENT | MODIFY_ORDER_EVENT;
pub const EXCH_FILL_EVENT: u64 = EXCH_EVENT | FILL_EVENT;
pub const UNTIL_END_OF_DATA: i64 = i64::MAX;

pub type OrderId = u64;

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum WaitOrderResponse {
    None,
    Any,
    Specified { asset_no: usize, order_id: OrderId },
}

#[repr(C, align(64))]
#[derive(Clone, PartialEq, Debug, Decode, Encode)]
pub struct Event {
    pub ev: u64,
    pub exch_ts: i64,
    pub local_ts: i64,
    pub px: f64,
    pub qty: f64,
    /// Reserved for normalized feed extensions. L2 Binance data sets this to zero.
    pub order_id: u64,
    pub ival: i64,
    pub fval: f64,
}

unsafe impl POD for Event {}

impl NpyDTyped for Event {
    fn descr() -> Vec<Field> {
        let endian = if cfg!(target_endian = "little") { "<" } else { ">" };
        [
            ("ev", "u8"),
            ("exch_ts", "i8"),
            ("local_ts", "i8"),
            ("px", "f8"),
            ("qty", "f8"),
            ("order_id", "u8"),
            ("ival", "i8"),
            ("fval", "f8"),
        ]
        .into_iter()
        .map(|(name, ty)| Field { name: name.to_string(), ty: format!("{endian}{ty}") })
        .collect()
    }
}

impl Event {
    #[inline(always)]
    pub fn is(&self, event: u64) -> bool {
        if (self.ev & event) != event {
            return false;
        }
        let event_kind = event & 0xff;
        event_kind == 0 || self.ev & 0xff == event_kind
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Decode, Encode)]
#[repr(i8)]
pub enum Side { Buy = 1, Sell = -1, None = 0, Unsupported = 127 }
impl AsRef<f64> for Side {
    fn as_ref(&self) -> &f64 {
        match self { Self::Buy => &1.0, Self::Sell => &-1.0, _ => panic!("unsupported side") }
    }
}
impl AsRef<str> for Side {
    fn as_ref(&self) -> &'static str {
        match self { Self::Buy => "BUY", Self::Sell => "SELL", _ => panic!("unsupported side") }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Decode, Encode)]
#[repr(u8)]
pub enum Status {
    None = 0, New = 1, Expired = 2, Filled = 3, Canceled = 4,
    PartiallyFilled = 5, Rejected = 6, Replaced = 7, Unsupported = 255,
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Decode, Encode)]
#[repr(u8)]
pub enum TimeInForce { GTC = 0, GTX = 1, FOK = 2, IOC = 3, Unsupported = 255 }
impl AsRef<str> for TimeInForce {
    fn as_ref(&self) -> &'static str {
        match self {
            Self::GTC => "GTC", Self::GTX => "GTX", Self::FOK => "FOK", Self::IOC => "IOC",
            Self::Unsupported => panic!("unsupported time in force"),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Decode, Encode)]
#[repr(u8)]
pub enum OrdType { Limit = 0, Market = 1, Unsupported = 255 }
impl AsRef<str> for OrdType {
    fn as_ref(&self) -> &'static str {
        match self { Self::Limit => "LIMIT", Self::Market => "MARKET", Self::Unsupported => panic!("unsupported order type") }
    }
}

pub trait AnyClone: DynClone {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
dyn_clone::clone_trait_object!(AnyClone);
impl AnyClone for () {
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}

#[derive(Clone)]
#[repr(C)]
pub struct Order {
    pub qty: f64,
    pub leaves_qty: f64,
    pub exec_qty: f64,
    pub exec_price_tick: i64,
    pub price_tick: i64,
    pub tick_size: f64,
    pub exch_timestamp: i64,
    pub local_timestamp: i64,
    pub order_id: u64,
    pub q: Box<dyn AnyClone + Send>,
    pub maker: bool,
    pub order_type: OrdType,
    pub req: Status,
    pub status: Status,
    pub side: Side,
    pub time_in_force: TimeInForce,
}

impl Order {
    pub fn new(
        order_id: u64,
        price_tick: i64,
        tick_size: f64,
        qty: f64,
        side: Side,
        order_type: OrdType,
        time_in_force: TimeInForce,
    ) -> Self {
        Self {
            qty, leaves_qty: qty, exec_qty: 0.0, exec_price_tick: 0, price_tick, tick_size,
            exch_timestamp: 0, local_timestamp: 0, order_id, q: Box::new(()), maker: false,
            order_type, req: Status::None, status: Status::None, side, time_in_force,
        }
    }
    pub fn price(&self) -> f64 { self.price_tick as f64 * self.tick_size }
    pub fn exec_price(&self) -> f64 { self.exec_price_tick as f64 * self.tick_size }
    pub fn cancellable(&self) -> bool {
        matches!(self.status, Status::New | Status::PartiallyFilled) && self.req == Status::None
    }
    pub fn active(&self) -> bool { matches!(self.status, Status::New | Status::PartiallyFilled) }
    pub fn pending(&self) -> bool { self.req != Status::None }
    pub fn update(&mut self, order: &Order) {
        self.qty = order.qty;
        self.leaves_qty = order.leaves_qty;
        self.price_tick = order.price_tick;
        self.tick_size = order.tick_size;
        self.side = order.side;
        self.time_in_force = order.time_in_force;
        if order.exch_timestamp > 0 { self.exch_timestamp = order.exch_timestamp; }
        self.status = order.status;
        self.req = order.req;
        self.exec_price_tick = order.exec_price_tick;
        self.exec_qty = order.exec_qty;
        self.order_id = order.order_id;
        self.q = order.q.clone();
        self.maker = order.maker;
        self.order_type = order.order_type;
    }
}

impl Debug for Order {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Order")
            .field("order_id", &self.order_id).field("side", &self.side)
            .field("qty", &self.qty).field("leaves_qty", &self.leaves_qty)
            .field("price_tick", &self.price_tick).field("status", &self.status)
            .field("req", &self.req).field("maker", &self.maker).finish()
    }
}

impl<Context> Decode<Context> for Order {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        Ok(Self {
            qty: Decode::decode(decoder)?, leaves_qty: Decode::decode(decoder)?, exec_qty: Decode::decode(decoder)?,
            exec_price_tick: Decode::decode(decoder)?, price_tick: Decode::decode(decoder)?, tick_size: Decode::decode(decoder)?,
            exch_timestamp: Decode::decode(decoder)?, local_timestamp: Decode::decode(decoder)?, order_id: Decode::decode(decoder)?,
            q: Box::new(()), maker: Decode::decode(decoder)?, order_type: Decode::decode(decoder)?, req: Decode::decode(decoder)?,
            status: Decode::decode(decoder)?, side: Decode::decode(decoder)?, time_in_force: Decode::decode(decoder)?,
        })
    }
}
impl<'de, Context> BorrowDecode<'de, Context> for Order {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
        Self::decode(decoder)
    }
}
impl Encode for Order {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.qty.encode(encoder)?; self.leaves_qty.encode(encoder)?; self.exec_qty.encode(encoder)?;
        self.exec_price_tick.encode(encoder)?; self.price_tick.encode(encoder)?; self.tick_size.encode(encoder)?;
        self.exch_timestamp.encode(encoder)?; self.local_timestamp.encode(encoder)?; self.order_id.encode(encoder)?;
        self.maker.encode(encoder)?; self.order_type.encode(encoder)?; self.req.encode(encoder)?;
        self.status.encode(encoder)?; self.side.encode(encoder)?; self.time_in_force.encode(encoder)?;
        Ok(())
    }
}

#[repr(C)]
#[derive(PartialEq, Clone, Debug, Default)]
pub struct StateValues {
    pub position: f64,
    pub balance: f64,
    pub fee: f64,
    pub num_trades: i64,
    pub trading_volume: f64,
    pub trading_value: f64,
}

#[derive(Error, Debug)]
pub enum BuildError {
    #[error("`{0}` is required")]
    BuilderIncomplete(&'static str),
    #[error("{0}")]
    InvalidArgument(&'static str),
    #[error("{0:?}")]
    Error(#[from] anyhow::Error),
}

#[derive(Decode, Encode)]
pub struct OrderRequest {
    pub order_id: u64,
    pub price: f64,
    pub qty: f64,
    pub side: Side,
    pub time_in_force: TimeInForce,
    pub order_type: OrdType,
}

pub trait Bot<MD>
where
    MD: MarketDepth,
{
    type Error;
    fn current_timestamp(&self) -> i64;
    fn num_assets(&self) -> usize;
    fn position(&self, asset_no: usize) -> f64;
    fn state_values(&self, asset_no: usize) -> &StateValues;
    fn depth(&self, asset_no: usize) -> &MD;
    fn last_trades(&self, asset_no: usize) -> &[Event];
    fn clear_last_trades(&mut self, asset_no: Option<usize>);
    fn orders(&self, asset_no: usize) -> &HashMap<OrderId, Order>;
    #[allow(clippy::too_many_arguments)]
    fn submit_buy_order(
        &mut self, asset_no: usize, order_id: OrderId, price: f64, qty: f64,
        time_in_force: TimeInForce, order_type: OrdType, wait: bool,
    ) -> Result<ElapseResult, Self::Error>;
    #[allow(clippy::too_many_arguments)]
    fn submit_sell_order(
        &mut self, asset_no: usize, order_id: OrderId, price: f64, qty: f64,
        time_in_force: TimeInForce, order_type: OrdType, wait: bool,
    ) -> Result<ElapseResult, Self::Error>;
    fn submit_order(&mut self, asset_no: usize, order: OrderRequest, wait: bool) -> Result<ElapseResult, Self::Error>;
    fn modify(&mut self, asset_no: usize, order_id: OrderId, price: f64, qty: f64, wait: bool) -> Result<ElapseResult, Self::Error>;
    fn cancel(&mut self, asset_no: usize, order_id: OrderId, wait: bool) -> Result<ElapseResult, Self::Error>;
    fn clear_inactive_orders(&mut self, asset_no: Option<usize>);
    fn wait_order_response(&mut self, asset_no: usize, order_id: OrderId, timeout: i64) -> Result<ElapseResult, Self::Error>;
    fn wait_next_feed(&mut self, include_order_resp: bool, timeout: i64) -> Result<ElapseResult, Self::Error>;
    fn elapse(&mut self, duration: i64) -> Result<ElapseResult, Self::Error>;
    fn elapse_bt(&mut self, duration: i64) -> Result<ElapseResult, Self::Error>;
    fn close(&mut self) -> Result<(), Self::Error>;
    fn feed_latency(&self, asset_no: usize) -> Option<(i64, i64)>;
    fn order_latency(&self, asset_no: usize) -> Option<(i64, i64, i64)>;
}

pub trait Recorder {
    type Error;
    fn record<MD, I>(&mut self, hbt: &I) -> Result<(), Self::Error>
    where I: Bot<MD>, MD: MarketDepth;
}

#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum ElapseResult { Ok, EndOfData, MarketFeed, OrderResponse }
