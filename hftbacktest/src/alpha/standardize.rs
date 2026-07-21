use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::FEATURE_COUNT;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeatureStandardizer {
    mean: Vec<f32>,
    scale: Vec<f32>,
}

impl FeatureStandardizer {
    pub fn fit(rows: &[[f32; FEATURE_COUNT]]) -> Result<Self, StandardizerError> {
        if rows.is_empty() {
            return Err(StandardizerError::Empty);
        }
        let mut mean = vec![0.0_f64; FEATURE_COUNT];
        for row in rows {
            for (sum, value) in mean.iter_mut().zip(row) {
                *sum += f64::from(*value);
            }
        }
        for value in &mut mean {
            *value /= rows.len() as f64;
        }
        let mut variance = vec![0.0_f64; FEATURE_COUNT];
        for row in rows {
            for ((sum, value), average) in variance.iter_mut().zip(row).zip(&mean) {
                *sum += (f64::from(*value) - average).powi(2);
            }
        }
        let scale = variance
            .into_iter()
            .map(|sum| {
                let std = (sum / rows.len() as f64).sqrt();
                if std <= f32::EPSILON as f64 {
                    1.0
                } else {
                    std as f32
                }
            })
            .collect();
        Ok(Self {
            mean: mean.into_iter().map(|value| value as f32).collect(),
            scale,
        })
    }

    pub fn identity() -> Self {
        Self {
            mean: vec![0.0; FEATURE_COUNT],
            scale: vec![1.0; FEATURE_COUNT],
        }
    }

    pub fn transform_value(&self, feature: usize, value: f32) -> f32 {
        (value - self.mean[feature]) / self.scale[feature]
    }
}

#[derive(Debug, Error)]
pub enum StandardizerError {
    #[error("cannot fit a standardizer on an empty training set")]
    Empty,
}
