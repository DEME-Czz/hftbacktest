use std::num::ParseFloatError;

use thiserror::Error;

/// Temporary parser error kept only while Binance parsing helpers are being separated
/// from the legacy multi-exchange utility module. No Bybit exchange implementation remains.
#[derive(Error, Debug)]
pub enum BybitError {
    #[error("invalid price or quantity: {0}")]
    InvalidPxQty(#[from] ParseFloatError),
}
