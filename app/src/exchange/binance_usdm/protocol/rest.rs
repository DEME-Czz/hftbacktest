use hftbacktest::types::{OrdType, Side, Status, TimeInForce};
use serde::Deserialize;

use super::{
    from_str_to_f64, from_str_to_side, from_str_to_status, from_str_to_tif, from_str_to_type,
    to_lowercase,
};

fn default_time_in_force() -> TimeInForce {
    TimeInForce::Unsupported
}

fn default_order_type() -> OrdType {
    OrdType::Unsupported
}

/// Binance 的下单、撤单和查询订单响应。
///
/// 注意：Binance 不同 REST 入口返回的订单字段并不完全一致，因此这里只要求交易核心
/// 真正需要且各订单详情响应稳定提供的字段。对于恢复流程不需要的字段使用默认值，
/// 避免 Testnet / Production 或不同 endpoint 的附加字段差异阻断执行链路。
#[derive(Deserialize, Debug)]
pub struct OrderResponse {
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
    #[serde(rename = "cumQty", default, deserialize_with = "from_str_to_f64")]
    pub cum_qty: f64,
    #[serde(rename = "executedQty", default, deserialize_with = "from_str_to_f64")]
    pub executed_qty: f64,
    #[serde(rename = "origQty", default, deserialize_with = "from_str_to_f64")]
    pub orig_qty: f64,
    #[serde(default, deserialize_with = "from_str_to_f64")]
    pub price: f64,
    #[serde(deserialize_with = "from_str_to_side")]
    pub side: Side,
    #[serde(deserialize_with = "from_str_to_status")]
    pub status: Status,
    #[serde(deserialize_with = "to_lowercase")]
    pub symbol: String,
    #[serde(
        rename = "timeInForce",
        default = "default_time_in_force",
        deserialize_with = "from_str_to_tif"
    )]
    pub time_in_force: TimeInForce,
    #[serde(
        rename = "type",
        default = "default_order_type",
        deserialize_with = "from_str_to_type"
    )]
    pub ty: OrdType,
    #[serde(rename = "updateTime", default)]
    pub update_time: i64,
}

/// `/fapi/v1/openOrders` 的最小 DTO。
///
/// openOrders 与 submit/query/cancel 的响应不是同一个 schema。例如 Testnet 的
/// openOrders 会返回 executedQty/origQty，但不返回 cumQty。账户恢复只需要这些字段，
/// 因此不能复用 OrderResponse 强制解析无关字段。
#[derive(Deserialize, Debug)]
pub struct OpenOrderResponse {
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
    #[serde(rename = "executedQty", default, deserialize_with = "from_str_to_f64")]
    pub executed_qty: f64,
    #[serde(rename = "origQty", default, deserialize_with = "from_str_to_f64")]
    pub orig_qty: f64,
    #[serde(default, deserialize_with = "from_str_to_f64")]
    pub price: f64,
    #[serde(deserialize_with = "from_str_to_side")]
    pub side: Side,
    #[serde(deserialize_with = "from_str_to_status")]
    pub status: Status,
    #[serde(deserialize_with = "to_lowercase")]
    pub symbol: String,
    #[serde(
        rename = "timeInForce",
        default = "default_time_in_force",
        deserialize_with = "from_str_to_tif"
    )]
    pub time_in_force: TimeInForce,
    #[serde(
        rename = "type",
        default = "default_order_type",
        deserialize_with = "from_str_to_type"
    )]
    pub ty: OrdType,
    #[serde(rename = "updateTime", default)]
    pub update_time: i64,
}

impl From<OpenOrderResponse> for OrderResponse {
    fn from(order: OpenOrderResponse) -> Self {
        Self {
            client_order_id: order.client_order_id,
            cum_qty: order.executed_qty,
            executed_qty: order.executed_qty,
            orig_qty: order.orig_qty,
            price: order.price,
            side: order.side,
            status: order.status,
            symbol: order.symbol,
            time_in_force: order.time_in_force,
            ty: order.ty,
            update_time: order.update_time,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct ErrorResponse {
    pub code: i64,
    pub msg: String,
}

/// 账户重新对账只需要 symbol、持仓方向、持仓数量和更新时间。
/// 不解码保证金、强平价等当前状态机未使用字段，避免 Binance 对附加字段的调整阻断恢复流程。
#[derive(Deserialize, Debug)]
pub struct PositionInformationV3 {
    #[serde(deserialize_with = "to_lowercase")]
    pub symbol: String,
    #[serde(rename = "positionSide")]
    pub position_side: String,
    #[serde(rename = "positionAmt", deserialize_with = "from_str_to_f64")]
    pub position_amount: f64,
    #[serde(rename = "updateTime", default)]
    pub update_time: i64,
}

#[derive(Deserialize, Debug)]
pub struct Depth {
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: i64,
    #[serde(rename = "E")]
    pub event_time: i64,
    #[serde(rename = "T")]
    pub transaction_time: i64,
    pub bids: Vec<(String, String)>,
    pub asks: Vec<(String, String)>,
}
