use thiserror::Error;

use super::{Direction, LobRecord, WINDOW_SIZE};

#[derive(Clone, Copy, Debug)]
pub struct LabelConfig {
    horizon: usize,
    threshold: f64,
}

impl LabelConfig {
    pub fn new(horizon: usize, threshold: f64) -> Result<Self, LabelConfigError> {
        if horizon == 0 {
            return Err(LabelConfigError::ZeroHorizon);
        }
        if !threshold.is_finite() || threshold < 0.0 {
            return Err(LabelConfigError::InvalidThreshold(threshold));
        }
        Ok(Self { horizon, threshold })
    }

    pub fn horizon(self) -> usize {
        self.horizon
    }

    pub fn threshold(self) -> f64 {
        self.threshold
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LabeledObservation {
    pub window_end_index: usize,
    pub direction: Direction,
    pub relative_move: f64,
}

/// Applies the paper's smoothed past/future mid-price labelling rule.
///
/// Only observations with a complete 100-state input window and a complete future horizon are
/// returned. The threshold should be calibrated to cover fees and expected execution costs.
pub fn label_records(records: &[LobRecord], config: LabelConfig) -> Vec<LabeledObservation> {
    let first_index = (WINDOW_SIZE - 1).max(config.horizon - 1);
    let Some(last_index) = records.len().checked_sub(config.horizon + 1) else {
        return Vec::new();
    };
    if first_index > last_index {
        return Vec::new();
    }

    (first_index..=last_index)
        .map(|index| {
            let past_start = index + 1 - config.horizon;
            let past_mean = mean_mid_price(&records[past_start..=index]);
            let future_mean = mean_mid_price(&records[index + 1..=index + config.horizon]);
            let relative_move = (future_mean - past_mean) / past_mean;
            let direction = if relative_move > config.threshold {
                Direction::Up
            } else if relative_move < -config.threshold {
                Direction::Down
            } else {
                Direction::Flat
            };
            LabeledObservation {
                window_end_index: index,
                direction,
                relative_move,
            }
        })
        .collect()
}

fn mean_mid_price(records: &[LobRecord]) -> f64 {
    records.iter().map(LobRecord::mid_price).sum::<f64>() / records.len() as f64
}

#[derive(Debug, Error)]
pub enum LabelConfigError {
    #[error("label horizon must be greater than zero")]
    ZeroHorizon,
    #[error("label threshold must be finite and non-negative, found {0}")]
    InvalidThreshold(f64),
}
