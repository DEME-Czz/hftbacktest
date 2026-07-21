use thiserror::Error;

use crate::depth::{INVALID_MAX, INVALID_MIN, MarketDepth};

pub const DEPTH_LEVELS: usize = 10;
pub const FEATURES_PER_LEVEL: usize = 4;
pub const FEATURE_COUNT: usize = DEPTH_LEVELS * FEATURES_PER_LEVEL;
const MAX_TICK_SCAN: usize = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub struct LobSnapshot {
    features: [f32; FEATURE_COUNT],
}

impl LobSnapshot {
    pub fn new(features: [f32; FEATURE_COUNT]) -> Result<Self, SnapshotError> {
        if features.iter().any(|value| !value.is_finite()) {
            return Err(SnapshotError::InvalidFeature);
        }
        Ok(Self { features })
    }

    pub fn from_depth(depth: &impl MarketDepth) -> Result<Self, SnapshotError> {
        if depth.best_bid_tick() == INVALID_MIN || depth.best_ask_tick() == INVALID_MAX {
            return Err(SnapshotError::Uninitialized);
        }
        if !depth.tick_size().is_finite() || depth.tick_size() <= 0.0 {
            return Err(SnapshotError::InvalidTickSize(depth.tick_size()));
        }

        let bids = collect_levels(depth, Side::Bid)?;
        let asks = collect_levels(depth, Side::Ask)?;
        let mut features = [0.0; FEATURE_COUNT];

        for level in 0..DEPTH_LEVELS {
            let offset = level * FEATURES_PER_LEVEL;
            features[offset] = asks[level].0;
            features[offset + 1] = asks[level].1;
            features[offset + 2] = bids[level].0;
            features[offset + 3] = bids[level].1;
        }

        Ok(Self { features })
    }

    pub fn features(&self) -> &[f32; FEATURE_COUNT] {
        &self.features
    }
}

#[derive(Clone, Copy)]
enum Side {
    Bid,
    Ask,
}

fn collect_levels(
    depth: &impl MarketDepth,
    side: Side,
) -> Result<[(f32, f32); DEPTH_LEVELS], SnapshotError> {
    let mut levels = [(0.0, 0.0); DEPTH_LEVELS];
    let mut found = 0;
    let start = match side {
        Side::Bid => depth.best_bid_tick(),
        Side::Ask => depth.best_ask_tick(),
    };

    for distance in 0..MAX_TICK_SCAN {
        let tick = match side {
            Side::Bid => start.checked_sub(distance as i64),
            Side::Ask => start.checked_add(distance as i64),
        }
        .ok_or(SnapshotError::TickOverflow)?;
        let qty = match side {
            Side::Bid => depth.bid_qty_at_tick(tick),
            Side::Ask => depth.ask_qty_at_tick(tick),
        };
        if qty == 0.0 {
            continue;
        }
        if !qty.is_finite() || qty < 0.0 {
            return Err(SnapshotError::InvalidQuantity { tick, qty });
        }

        let price = tick as f64 * depth.tick_size();
        if !price.is_finite() {
            return Err(SnapshotError::InvalidPrice { tick, price });
        }
        levels[found] = (price as f32, qty as f32);
        found += 1;
        if found == DEPTH_LEVELS {
            return Ok(levels);
        }
    }

    Err(match side {
        Side::Bid => SnapshotError::InsufficientBidLevels { found },
        Side::Ask => SnapshotError::InsufficientAskLevels { found },
    })
}

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("order book is not initialized")]
    Uninitialized,
    #[error("invalid tick size: {0}")]
    InvalidTickSize(f64),
    #[error("only {found} bid levels were found")]
    InsufficientBidLevels { found: usize },
    #[error("only {found} ask levels were found")]
    InsufficientAskLevels { found: usize },
    #[error("invalid quantity {qty} at tick {tick}")]
    InvalidQuantity { tick: i64, qty: f64 },
    #[error("invalid price {price} at tick {tick}")]
    InvalidPrice { tick: i64, price: f64 },
    #[error("price tick overflow")]
    TickOverflow,
    #[error("snapshot contains a non-finite feature")]
    InvalidFeature,
}
