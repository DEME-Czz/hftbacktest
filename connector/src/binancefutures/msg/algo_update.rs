use serde::Deserialize;

use crate::utils::to_lowercase;

#[derive(Deserialize, Debug)]
pub struct AlgoUpdate {
    #[serde(rename = "E")]
    pub event_time: i64,
    #[serde(rename = "T")]
    pub transaction_time: i64,
    #[serde(rename = "o")]
    pub order: AlgoOrder,
}

#[derive(Deserialize, Debug)]
pub struct AlgoOrder {
    #[serde(rename = "caid")]
    pub client_algo_id: String,
    #[serde(rename = "s", deserialize_with = "to_lowercase")]
    pub symbol: String,
    #[serde(rename = "X")]
    pub status: String,
    #[serde(rename = "R")]
    pub reduce_only: bool,
}
