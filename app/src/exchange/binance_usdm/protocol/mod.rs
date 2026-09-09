use std::fmt;

use hftbacktest::types::{OrdType, Side, Status, TimeInForce};
use serde::{
    Deserialize, Deserializer,
    de::{self, Error, Unexpected, Visitor},
};

#[allow(dead_code)]
pub mod rest;
#[allow(dead_code)]
pub mod stream;

fn from_str_to_side<'de, D>(deserializer: D) -> Result<Side, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    match s.as_str() {
        "BUY" => Ok(Side::Buy),
        "SELL" => Ok(Side::Sell),
        value => Err(Error::invalid_value(
            Unexpected::Other(value),
            &"BUY or SELL",
        )),
    }
}

fn from_str_to_status<'de, D>(deserializer: D) -> Result<Status, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(match s.as_str() {
        "NEW" => Status::New,
        "PARTIALLY_FILLED" => Status::PartiallyFilled,
        "FILLED" => Status::Filled,
        "CANCELED" => Status::Canceled,
        "REJECTED" => Status::Rejected,
        "EXPIRED" | "EXPIRED_IN_MATCH" => Status::Expired,
        _ => Status::Unsupported,
    })
}

/// The engine intentionally trades only LIMIT/MARKET. New or external Binance order types are
/// decoded as `Unsupported` so one unknown order does not break the entire user-data stream.
fn from_str_to_type<'de, D>(deserializer: D) -> Result<OrdType, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(match s.as_str() {
        "LIMIT" => OrdType::Limit,
        "MARKET" => OrdType::Market,
        _ => OrdType::Unsupported,
    })
}

/// GTD/RPI are current Binance values but are outside this engine's execution surface. Decode
/// them as `Unsupported` rather than failing deserialization of account/order events.
fn from_str_to_tif<'de, D>(deserializer: D) -> Result<TimeInForce, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(match s.as_str() {
        "GTC" => TimeInForce::GTC,
        "IOC" => TimeInForce::IOC,
        "FOK" => TimeInForce::FOK,
        "GTX" => TimeInForce::GTX,
        _ => TimeInForce::Unsupported,
    })
}

struct F64Visitor;

impl Visitor<'_> for F64Visitor {
    type Value = Option<f64>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string containing an f64 number")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.is_empty() {
            Ok(None)
        } else {
            value.parse::<f64>().map(Some).map_err(Error::custom)
        }
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }
}

pub fn from_str_to_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer
        .deserialize_string(F64Visitor)
        .map(|value| value.unwrap_or(0.0))
}

pub fn to_lowercase<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(String::deserialize(deserializer)?.to_lowercase())
}

pub type PxQty = (f64, f64);

pub fn parse_px_qty(px: String, qty: String) -> Result<PxQty, std::num::ParseFloatError> {
    Ok((px.parse()?, qty.parse()?))
}

pub fn parse_depth(
    bids: Vec<(String, String)>,
    asks: Vec<(String, String)>,
) -> Result<(Vec<PxQty>, Vec<PxQty>), std::num::ParseFloatError> {
    let bids = bids
        .into_iter()
        .map(|(px, qty)| parse_px_qty(px, qty))
        .collect::<Result<Vec<_>, _>>()?;
    let asks = asks
        .into_iter()
        .map(|(px, qty)| parse_px_qty(px, qty))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((bids, asks))
}
