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

/// Binance 的订单响应。
///
/// 不同 REST 入口的字段并不完全一致。例如 openOrders 在 Testnet 中不返回 cumQty，
/// 因此恢复流程不依赖的数值字段允许缺失并使用默认值。真正影响订单身份和状态的
/// clientOrderId / side / status / symbol 仍保持必填。
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

#[derive(Deserialize, Debug)]
pub struct ErrorResponse {
    pub code: i64,
    pub msg: String,
}

/// `/fapi/v1/openOrders` 可能返回订单数组或 Binance 错误对象。
/// OrderResponse 已对 openOrders 不提供的 cumQty 等非核心字段做兼容处理。
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum OpenOrdersResponse {
    Ok(Vec<OrderResponse>),
    Err(ErrorResponse),
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
