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
    #[serde(rename = "cumQty")]
    #[serde(deserialize_with = "from_str_to_f64")]
    pub cum_qty: f64,
    /// New Order and Cancel Order responses only field
    #[serde(rename = "cumQuote")]
    #[serde(default)]
    #[serde(deserialize_with = "from_str_to_f64_opt")]
    pub cum_quote: Option<f64>,
    /// Modify Order response only field
    #[serde(rename = "cumBase")]
    #[serde(default)]
    #[serde(deserialize_with = "from_str_to_f64_opt")]
    pub cum_base: Option<f64>,
    #[serde(rename = "executedQty")]
    #[serde(deserialize_with = "from_str_to_f64")]
    pub executed_qty: f64,
    #[serde(rename = "orderId")]
    pub order_id: i64,
    /// New Order and Modify Order responses only field
    #[serde(rename = "avgPrice")]
    #[serde(default)]
    #[serde(deserialize_with = "from_str_to_f64_opt")]
    pub avg_price: Option<f64>,
    #[serde(rename = "origQty")]
    #[serde(deserialize_with = "from_str_to_f64")]
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
    #[serde(rename = "stopPrice")]
    #[serde(deserialize_with = "from_str_to_f64")]
    pub stop_price: f64,
    #[serde(rename = "closePosition")]
    pub close_position: bool,
    #[serde(deserialize_with = "to_lowercase")]
    pub symbol: String,
    // for Coin-M futures
    // pub pair: String,
    /// Modify Order response only field
    #[serde(default)]
    pub pair: Option<String>,
    #[serde(rename = "timeInForce")]
    #[serde(deserialize_with = "from_str_to_tif")]
    pub time_in_force: TimeInForce,
    #[serde(rename = "type")]
    #[serde(deserialize_with = "from_str_to_type")]
    pub ty: OrdType,
    #[serde(rename = "origType")]
    #[serde(deserialize_with = "from_str_to_type")]
    pub orig_type: OrdType,
    /// New Order and Cancel Order responses only field
    #[serde(rename = "activatePrice")]
    #[serde(default)]
    #[serde(deserialize_with = "from_str_to_f64_opt")]
    pub activate_price: Option<f64>,
    /// New Order and Cancel Order responses only field
    #[serde(rename = "priceRate")]
    #[serde(default)]
    #[serde(deserialize_with = "from_str_to_f64_opt")]
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
}

#[derive(Deserialize, Debug)]
pub struct ErrorResponse {
    pub code: i64,
    pub msg: String,
}

#[derive(Deserialize, Debug)]
pub struct PositionInformationV3 {
    #[serde(deserialize_with = "to_lowercase")]
    pub symbol: String,
    #[serde(rename = "positionAmt")]
    #[serde(deserialize_with = "from_str_to_f64")]
    pub position_amount: f64,
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

#[cfg(test)]
mod tests {
    use super::PositionInformationV3;

    #[test]
    fn deserializes_position_information_v3_without_v2_only_fields() {
        let json = r#"{
            "symbol":"DOGEUSDT",
            "positionSide":"BOTH",
            "positionAmt":"125",
            "entryPrice":"0.125",
            "breakEvenPrice":"0.1251",
            "markPrice":"0.126",
            "unRealizedProfit":"0.125",
            "liquidationPrice":"0",
            "isolatedMargin":"0",
            "notional":"15.75",
            "marginAsset":"USDT",
            "isolatedWallet":"0",
            "initialMargin":"0.7875",
            "maintMargin":"0.1024",
            "positionInitialMargin":"0.7875",
            "openOrderInitialMargin":"0",
            "adl":2,
            "bidNotional":"0",
            "askNotional":"0",
            "updateTime":1720736417660
        }"#;

        let position: PositionInformationV3 = serde_json::from_str(json).unwrap();

        assert_eq!(position.symbol, "dogeusdt");
        assert_eq!(position.position_amount, 125.0);
        assert_eq!(position.update_time, 1_720_736_417_660);
    }
}
