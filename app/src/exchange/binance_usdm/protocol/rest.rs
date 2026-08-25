use hftbacktest::types::{OrdType, Side, Status, TimeInForce};
use serde::Deserialize;

use super::{
    from_str_to_f64, from_str_to_f64_opt, from_str_to_side, from_str_to_status, from_str_to_tif,
    from_str_to_type, to_lowercase,
};

fn default_order_type() -> OrdType {
    OrdType::Unsupported
}

fn default_time_in_force() -> TimeInForce {
    TimeInForce::Unsupported
}

fn default_side() -> Side {
    Side::Unsupported
}

fn default_status() -> Status {
    Status::Unsupported
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum OpenOrdersResponse {
    Ok(Vec<OrderResponse>),
    Err(ErrorResponse),
}

/// Binance Testnet and production occasionally differ in auxiliary order-response fields.
/// Decode the wire response permissively here; callers validate the small set of fields that are
/// actually required to mutate the local order state before accepting the response.
#[derive(Deserialize, Debug)]
pub struct OrderResponse {
    #[serde(rename = "clientOrderId", default)]
    pub client_order_id: String,
    #[serde(rename = "cumQty", default, deserialize_with = "from_str_to_f64")]
    pub cum_qty: f64,
    #[serde(rename = "cumQuote", default, deserialize_with = "from_str_to_f64_opt")]
    pub cum_quote: Option<f64>,
    #[serde(rename = "cumBase", default, deserialize_with = "from_str_to_f64_opt")]
    pub cum_base: Option<f64>,
    #[serde(rename = "executedQty", default, deserialize_with = "from_str_to_f64")]
    pub executed_qty: f64,
    #[serde(rename = "orderId", default)]
    pub order_id: i64,
    #[serde(rename = "avgPrice", default, deserialize_with = "from_str_to_f64_opt")]
    pub avg_price: Option<f64>,
    #[serde(rename = "origQty", default, deserialize_with = "from_str_to_f64")]
    pub orig_qty: f64,
    #[serde(default, deserialize_with = "from_str_to_f64")]
    pub price: f64,
    #[serde(rename = "reduceOnly", default)]
    pub reduce_only: bool,
    #[serde(default = "default_side", deserialize_with = "from_str_to_side")]
    pub side: Side,
    #[serde(rename = "positionSide", default)]
    pub position_side: String,
    #[serde(default = "default_status", deserialize_with = "from_str_to_status")]
    pub status: Status,
    #[serde(rename = "stopPrice", default, deserialize_with = "from_str_to_f64")]
    pub stop_price: f64,
    #[serde(rename = "closePosition", default)]
    pub close_position: bool,
    #[serde(default, deserialize_with = "to_lowercase")]
    pub symbol: String,
    #[serde(default)]
    pub pair: Option<String>,
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
    #[serde(
        rename = "origType",
        default = "default_order_type",
        deserialize_with = "from_str_to_type"
    )]
    pub orig_type: OrdType,
    #[serde(
        rename = "activatePrice",
        default,
        deserialize_with = "from_str_to_f64_opt"
    )]
    pub activate_price: Option<f64>,
    #[serde(
        rename = "priceRate",
        default,
        deserialize_with = "from_str_to_f64_opt"
    )]
    pub price_rate: Option<f64>,
    #[serde(rename = "updateTime", default)]
    pub update_time: i64,
    #[serde(rename = "workingType", default)]
    pub working_type: String,
    #[serde(rename = "priceProtect", default)]
    pub price_protect: bool,
    #[serde(rename = "priceMatch", default)]
    pub price_match: String,
    #[serde(rename = "selfTradePreventionMode", default)]
    pub self_trade_prevention_mode: String,
    #[serde(rename = "goodTillDate", default)]
    pub good_till_date: i64,
    /// Optional pass-through identifier added to modify-order responses in 2026-07.
    #[serde(rename = "modifyId", default)]
    pub modify_id: Option<i64>,
}

#[derive(Deserialize, Debug)]
pub struct ErrorResponse {
    pub code: i64,
    pub msg: String,
}

/// Current USD-M Position Information schema (`GET /fapi/v3/positionRisk`).
#[derive(Deserialize, Debug)]
pub struct PositionInformationV3 {
    #[serde(deserialize_with = "to_lowercase")]
    pub symbol: String,
    #[serde(rename = "positionSide")]
    pub position_side: String,
    #[serde(rename = "positionAmt", deserialize_with = "from_str_to_f64")]
    pub position_amount: f64,
    #[serde(rename = "entryPrice", deserialize_with = "from_str_to_f64")]
    pub entry_price: f64,
    #[serde(rename = "breakEvenPrice", deserialize_with = "from_str_to_f64")]
    pub breakeven_price: f64,
    #[serde(rename = "markPrice", deserialize_with = "from_str_to_f64")]
    pub mark_price: f64,
    #[serde(rename = "unRealizedProfit", deserialize_with = "from_str_to_f64")]
    pub unrealized_pnl: f64,
    #[serde(rename = "liquidationPrice", deserialize_with = "from_str_to_f64")]
    pub liquidation_price: f64,
    #[serde(rename = "isolatedMargin", deserialize_with = "from_str_to_f64")]
    pub isolated_margin: f64,
    #[serde(deserialize_with = "from_str_to_f64")]
    pub notional: f64,
    #[serde(rename = "marginAsset")]
    pub margin_asset: String,
    #[serde(rename = "isolatedWallet", deserialize_with = "from_str_to_f64")]
    pub isolated_wallet: f64,
    #[serde(rename = "initialMargin", deserialize_with = "from_str_to_f64")]
    pub initial_margin: f64,
    #[serde(rename = "maintMargin", deserialize_with = "from_str_to_f64")]
    pub maint_margin: f64,
    #[serde(rename = "positionInitialMargin", deserialize_with = "from_str_to_f64")]
    pub position_initial_margin: f64,
    #[serde(
        rename = "openOrderInitialMargin",
        deserialize_with = "from_str_to_f64"
    )]
    pub open_order_initial_margin: f64,
    pub adl: i64,
    #[serde(rename = "bidNotional", deserialize_with = "from_str_to_f64")]
    pub bid_notional: f64,
    #[serde(rename = "askNotional", deserialize_with = "from_str_to_f64")]
    pub ask_notional: f64,
    #[serde(rename = "updateTime")]
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
