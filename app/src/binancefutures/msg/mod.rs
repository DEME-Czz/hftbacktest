use hftbacktest::types::{OrdType, Side, Status, TimeInForce};
use serde::{Deserialize, Deserializer, de::{Error, Unexpected}};

#[allow(dead_code)]
pub mod rest;
#[allow(dead_code)]
pub mod stream;

fn from_str_to_side<'de, D>(deserializer: D) -> Result<Side, D::Error>
where
    D: Deserializer<'de>,
{
    let s: &str = Deserialize::deserialize(deserializer)?;
    match s {
        "BUY" => Ok(Side::Buy),
        "SELL" => Ok(Side::Sell),
        s => Err(Error::invalid_value(Unexpected::Other(s), &"BUY or SELL")),
    }
}

fn from_str_to_status<'de, D>(deserializer: D) -> Result<Status, D::Error>
where
    D: Deserializer<'de>,
{
    let s: &str = Deserialize::deserialize(deserializer)?;
    match s {
        "NEW" => Ok(Status::New),
        "PARTIALLY_FILLED" => Ok(Status::PartiallyFilled),
        "FILLED" => Ok(Status::Filled),
        "CANCELED" => Ok(Status::Canceled),
        "REJECTED" => Ok(Status::Rejected),
        "EXPIRED" | "EXPIRED_IN_MATCH" => Ok(Status::Expired),
        _ => Ok(Status::Unsupported),
    }
}

/// The engine intentionally trades only LIMIT/MARKET. New or external Binance order types are
/// decoded as `Unsupported` so one unknown order does not break the entire user-data stream.
fn from_str_to_type<'de, D>(deserializer: D) -> Result<OrdType, D::Error>
where
    D: Deserializer<'de>,
{
    let s: &str = Deserialize::deserialize(deserializer)?;
    Ok(match s {
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
    let s: &str = Deserialize::deserialize(deserializer)?;
    Ok(match s {
        "GTC" => TimeInForce::GTC,
        "IOC" => TimeInForce::IOC,
        "FOK" => TimeInForce::FOK,
        "GTX" => TimeInForce::GTX,
        _ => TimeInForce::Unsupported,
    })
}
