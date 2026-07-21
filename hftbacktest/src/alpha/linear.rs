use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    AlphaModel, AlphaPrediction, FEATURE_COUNT, FeatureStandardizer, LobWindow, WINDOW_SIZE,
};

pub const LINEAR_INPUT_COUNT: usize = FEATURE_COUNT * 3;
const CLASS_COUNT: usize = 3;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinearAlphaModel {
    scaler: FeatureStandardizer,
    weights: Vec<f32>,
    bias: [f32; CLASS_COUNT],
}

impl LinearAlphaModel {
    pub fn new(
        scaler: FeatureStandardizer,
        weights: Vec<f32>,
        bias: [f32; CLASS_COUNT],
    ) -> Result<Self, LinearModelError> {
        if weights.len() != CLASS_COUNT * LINEAR_INPUT_COUNT {
            return Err(LinearModelError::WeightCount(weights.len()));
        }
        if weights
            .iter()
            .chain(bias.iter())
            .any(|value| !value.is_finite())
        {
            return Err(LinearModelError::NonFinite);
        }
        Ok(Self {
            scaler,
            weights,
            bias,
        })
    }

    pub fn to_json(&self) -> Result<String, LinearModelError> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(json: &str) -> Result<Self, LinearModelError> {
        let model: Self = serde_json::from_str(json)?;
        Self::new(model.scaler, model.weights, model.bias)
    }
}

impl AlphaModel for LinearAlphaModel {
    type Error = LinearModelError;

    fn predict(&mut self, input: &LobWindow) -> Result<AlphaPrediction, Self::Error> {
        if !input.is_ready() {
            return Err(LinearModelError::WindowNotReady(input.len()));
        }
        let mut logits = self.bias;
        let features = window_features(input, &self.scaler)?;
        for (input_index, normalized) in features.iter().enumerate() {
            for (class, logit) in logits.iter_mut().enumerate() {
                *logit += self.weights[class * LINEAR_INPUT_COUNT + input_index] * normalized;
            }
        }
        let max = logits.into_iter().fold(f32::NEG_INFINITY, f32::max);
        let exp = logits.map(|value| (value - max).exp());
        let sum = exp.iter().sum::<f32>();
        AlphaPrediction::new(exp[0] / sum, exp[1] / sum, exp[2] / sum)
            .map_err(|_| LinearModelError::InvalidPrediction)
    }
}

pub(crate) fn window_features(
    input: &LobWindow,
    scaler: &FeatureStandardizer,
) -> Result<[f32; LINEAR_INPUT_COUNT], LinearModelError> {
    if !input.is_ready() {
        return Err(LinearModelError::WindowNotReady(input.len()));
    }
    let first = input
        .iter()
        .next()
        .ok_or(LinearModelError::WindowNotReady(0))?;
    let latest = input.latest().ok_or(LinearModelError::WindowNotReady(0))?;
    let mut result = [0.0; LINEAR_INPUT_COUNT];
    for snapshot in input.iter() {
        for (feature, value) in snapshot.features().iter().enumerate() {
            result[feature] += scaler.transform_value(feature, *value) / WINDOW_SIZE as f32;
        }
    }
    for feature in 0..FEATURE_COUNT {
        let first_value = scaler.transform_value(feature, first.features()[feature]);
        let latest_value = scaler.transform_value(feature, latest.features()[feature]);
        result[FEATURE_COUNT + feature] = latest_value;
        result[2 * FEATURE_COUNT + feature] = latest_value - first_value;
    }
    Ok(result.map(|value| value.clamp(-10.0, 10.0)))
}

#[derive(Debug, Error)]
pub enum LinearModelError {
    #[error("expected 360 weights, found {0}")]
    WeightCount(usize),
    #[error("model contains a non-finite parameter")]
    NonFinite,
    #[error("expected a complete window, found {0} states")]
    WindowNotReady(usize),
    #[error("model produced invalid class probabilities")]
    InvalidPrediction,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
