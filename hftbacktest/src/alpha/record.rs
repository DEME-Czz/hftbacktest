use thiserror::Error;

use crate::depth::MarketDepth;

use super::{LobSnapshot, SnapshotError};

#[derive(Clone, Debug)]
pub struct LobRecord {
    exchange_timestamp: i64,
    mid_price: f64,
    snapshot: LobSnapshot,
}

impl LobRecord {
    pub fn from_depth(
        exchange_timestamp: i64,
        depth: &impl MarketDepth,
    ) -> Result<Self, RecordError> {
        let mid_price = (depth.best_bid() + depth.best_ask()) / 2.0;
        let snapshot = LobSnapshot::from_depth(depth)?;
        Self::new(exchange_timestamp, mid_price, snapshot)
    }

    pub fn new(
        exchange_timestamp: i64,
        mid_price: f64,
        snapshot: LobSnapshot,
    ) -> Result<Self, RecordError> {
        if !mid_price.is_finite() || mid_price <= 0.0 {
            return Err(RecordError::InvalidMidPrice(mid_price));
        }
        Ok(Self {
            exchange_timestamp,
            mid_price,
            snapshot,
        })
    }

    pub fn exchange_timestamp(&self) -> i64 {
        self.exchange_timestamp
    }

    pub fn mid_price(&self) -> f64 {
        self.mid_price
    }

    pub fn snapshot(&self) -> &LobSnapshot {
        &self.snapshot
    }
}

#[derive(Debug, Error)]
pub enum RecordError {
    #[error("invalid mid price: {0}")]
    InvalidMidPrice(f64),
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
}
