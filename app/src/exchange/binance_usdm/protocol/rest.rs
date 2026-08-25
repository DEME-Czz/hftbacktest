use hftbacktest::types::{OrdType, Side, Status, TimeInForce};
use serde::Deserialize;

use super::{
    from_str_to_f64, from_str_to_side, from_str_to_status, from_str_to_tif, from_str_to_type,
    to_lowercase,
};

fn default_time_in_force() -> TimeInForce {
    TimeInForce::Unsupported
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum OpenOrdersResponse {
    Ok(Vec<OrderResponse>),
    Err(ErrorResponse),
}

/// Binance 的订单响应在 Testnet、生产环境及不同 REST 入口之间会携带不同的附加字段。
/// 本地订单状态机只解码真正用于订单恢复和状态推进的最小字段集合；其余 Binance 字段
/// 由 Serde 自动忽略，避免非核心字段变化导致整个交易执行链路反序列化失败。
#[derive(Deserialize, Debug)]
pub struct OrderResponse {
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
    #[serde(rename = "cumQty", deserialize_with = "from_str_to_f64")]
    pub cum_qty: f64,
    #[serde(rename = "executedQty", deserialize_with = "from_str_to_f64")]
    pub executed_qty: f64,
    #[serde(rename = "origQty", deserialize_with = "from_str_to_f64")]
    pub orig_qty: f64,
    #[serde(deserialize_with = "from_str_to_f64")]
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
    #[serde(rename = "type", deserialize_with = "from_str_to_type")]
    pub ty: OrdType,
    #[serde(rename = "updateTime", default)]
    pub update_time: i64,
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
