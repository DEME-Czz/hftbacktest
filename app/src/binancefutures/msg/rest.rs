use hftbacktest::types::{OrdType, Side, Status, TimeInForce};
use serde::Deserialize;

use super::{from_str_to_side, from_str_to_status, from_str_to_tif, from_str_to_type};
use crate::utils::{from_str_to_f64, from_str_to_f64_opt, to_lowercase};

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum OrderResponseResult {
    Ok(OrderResponse),
    Err(ErrorResponse),
}

#[derive(Deserialize, Debug)]
pub struct OrderResponse {
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
    #[serde(rename = "cumQty", deserialize_with = "from_str_to_f64")]
    pub cum_qty: f64,
    #[serde(rename = "cumQuote", default, deserialize_with = "from_str_to_f64_opt")]
    pub cum_quote: Option<f64>,
    #[serde(rename = "cumBase", default, deserialize_with = "from_str_to_f64_opt")]
    pub cum_base: Option<f64>,
    #[serde(rename = "executedQty", deserialize_with = "from_str_to_f64")]
    pub executed_qty: f64,
    #[serde(rename = "orderId")]
    pub order_id: i64,
    #[serde(rename = "avgPrice", default, deserialize_with = "from_str_to_f64_opt")]
    pub avg_price: Option<f64>,
    #[serde(rename = "origQty", deserialize_with = "from_str_to_f64")]
    pub orig_qty: f64,
    #[serde(deserialize_with = "from_str_to_f64")]
    pub price: f64,
    #[serde(rename = "reduceOnly")]
    pub reduce_only: bool,
    #[serde(deserialize_with = "from_str_to_side")]
    pub side: Side,
    #[serde(rename = "positionSide")]
    pub position_side: String,
    #[serde(deserialize_with = "from_str_to_status")]
    pub status: Status,
    #[serde(rename = "stopPrice", deserialize_with = "from_str_to_f64")]
    pub stop_price: f64,
    #[serde(rename = "closePosition")]
    pub close_position: bool,
    #[serde(deserialize_with = "to_lowercase")]
    pub symbol: String,
    #[serde(default)]
    pub pair: Option<String>,
    #[serde(rename = "timeInForce", deserialize_with = "from_str_to_tif")]
    pub time_in_force: TimeInForce,
    #[serde(rename = "type", deserialize_with = "from_str_to_type")]
    pub ty: OrdType,
    #[serde(rename = "origType", deserialize_with = "from_str_to_type")]
    pub orig_type: OrdType,
    #[serde(rename = "activatePrice", default, deserialize_with = "from_str_to_f64_opt")]
    pub activate_price: Option<f64>,
    #[serde(rename = "priceRate", default, deserialize_with = "from_str_to_f64_opt")]
    pub price_rate: Option<f64>,
    #[serde(rename = "updateTime")]
    pub update_time: i64,
    #[serde(rename = "workingType")]
    pub working_type: String,
    #[serde(rename = "priceProtect")]
    pub price_protect: bool,
    #[serde(rename = "priceMatch")]
    pub price_match: String,
    #[serde(rename = "selfTradePreventionMode")]
    pub self_trade_prevention_mode: String,
    #[serde(rename = "goodTillDate")]
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
    #[serde(rename = "openOrderInitialMargin", deserialize_with = "from_str_to_f64")]
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
