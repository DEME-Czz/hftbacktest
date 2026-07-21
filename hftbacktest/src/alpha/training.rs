use thiserror::Error;

use super::{
    Direction, FEATURE_COUNT, FeatureStandardizer, LINEAR_INPUT_COUNT, LabelConfig,
    LinearAlphaModel, LobRecord, WINDOW_SIZE, label_records,
};

const CLASS_COUNT: usize = 3;

#[derive(Clone, Copy, Debug)]
pub struct TrainingConfig {
    pub train_ratio: f64,
    pub epochs: usize,
    pub learning_rate: f32,
    pub l2: f32,
}

impl TrainingConfig {
    pub fn new(
        train_ratio: f64,
        epochs: usize,
        learning_rate: f32,
        l2: f32,
    ) -> Result<Self, TrainingError> {
        if !train_ratio.is_finite()
            || train_ratio <= 0.0
            || train_ratio >= 1.0
            || epochs == 0
            || !learning_rate.is_finite()
            || learning_rate <= 0.0
            || !l2.is_finite()
            || l2 < 0.0
        {
            return Err(TrainingError::InvalidConfig);
        }
        Ok(Self {
            train_ratio,
            epochs,
            learning_rate,
            l2,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TrainingReport {
    pub records: usize,
    pub train_samples: usize,
    pub validation_samples: usize,
    pub train_class_counts: [usize; CLASS_COUNT],
    pub validation_class_counts: [usize; CLASS_COUNT],
    pub confusion: [[usize; CLASS_COUNT]; CLASS_COUNT],
    pub validation_accuracy: f64,
}

pub fn train_linear_model(
    records: &[LobRecord],
    labels: LabelConfig,
    config: TrainingConfig,
) -> Result<(LinearAlphaModel, TrainingReport), TrainingError> {
    let split = (records.len() as f64 * config.train_ratio) as usize;
    if split < WINDOW_SIZE || records.len().saturating_sub(split) < WINDOW_SIZE + labels.horizon() {
        return Err(TrainingError::InsufficientRecords(records.len()));
    }
    let observations = label_records(records, labels);
    let train: Vec<_> = observations
        .iter()
        .filter(|item| item.window_end_index + labels.horizon() < split)
        .collect();
    let validation: Vec<_> = observations
        .iter()
        .filter(|item| item.window_end_index >= split + WINDOW_SIZE - 1)
        .collect();
    if train.is_empty() || validation.is_empty() {
        return Err(TrainingError::InsufficientRecords(records.len()));
    }

    let scaler_rows: Vec<_> = records[..split]
        .iter()
        .map(|record| *record.snapshot().features())
        .collect();
    let scaler = FeatureStandardizer::fit(&scaler_rows)?;
    let mut train_counts = [0_usize; CLASS_COUNT];
    for item in &train {
        train_counts[class_index(item.direction)] += 1;
    }
    if train_counts.contains(&0) {
        return Err(TrainingError::MissingTrainingClass(train_counts));
    }
    let class_weights =
        train_counts.map(|count| (train.len() as f32 / (CLASS_COUNT as f32 * count as f32)).sqrt());
    let mut weights = vec![0.0_f32; CLASS_COUNT * LINEAR_INPUT_COUNT];
    let mut bias = [0.0_f32; CLASS_COUNT];

    for _ in 0..config.epochs {
        let mut weight_gradient = vec![0.0_f32; weights.len()];
        let mut bias_gradient = [0.0_f32; CLASS_COUNT];
        for item in &train {
            let x = observation_features(records, item.window_end_index, &scaler);
            let probabilities = probabilities(&weights, bias, &x);
            let expected = class_index(item.direction);
            for class in 0..CLASS_COUNT {
                let error = (probabilities[class] - if class == expected { 1.0 } else { 0.0 })
                    * class_weights[expected];
                bias_gradient[class] += error;
                for input in 0..LINEAR_INPUT_COUNT {
                    let index = class * LINEAR_INPUT_COUNT + input;
                    weight_gradient[index] += error * x[input];
                }
            }
        }
        let sample_count = train.len() as f32;
        for class in 0..CLASS_COUNT {
            bias[class] -= config.learning_rate * bias_gradient[class] / sample_count;
        }
        for (weight, gradient) in weights.iter_mut().zip(weight_gradient) {
            *weight -= config.learning_rate * (gradient / sample_count + config.l2 * *weight);
        }
    }

    let mut validation_counts = [0_usize; CLASS_COUNT];
    let mut confusion = [[0_usize; CLASS_COUNT]; CLASS_COUNT];
    for item in &validation {
        let expected = class_index(item.direction);
        let predicted = deployed_class(probabilities(
            &weights,
            bias,
            &observation_features(records, item.window_end_index, &scaler),
        ));
        validation_counts[expected] += 1;
        confusion[expected][predicted] += 1;
    }
    let correct = (0..CLASS_COUNT)
        .map(|class| confusion[class][class])
        .sum::<usize>();
    let report = TrainingReport {
        records: records.len(),
        train_samples: train.len(),
        validation_samples: validation.len(),
        train_class_counts: train_counts,
        validation_class_counts: validation_counts,
        confusion,
        validation_accuracy: correct as f64 / validation.len() as f64,
    };
    Ok((LinearAlphaModel::new(scaler, weights, bias)?, report))
}

fn observation_features(
    records: &[LobRecord],
    end: usize,
    scaler: &FeatureStandardizer,
) -> [f32; LINEAR_INPUT_COUNT] {
    let window = &records[end + 1 - WINDOW_SIZE..=end];
    let mut result = [0.0; LINEAR_INPUT_COUNT];
    for record in window {
        for (feature, value) in record.snapshot().features().iter().enumerate() {
            result[feature] += scaler.transform_value(feature, *value) / WINDOW_SIZE as f32;
        }
    }
    for feature in 0..FEATURE_COUNT {
        let first = scaler.transform_value(feature, window[0].snapshot().features()[feature]);
        let latest = scaler.transform_value(
            feature,
            window[WINDOW_SIZE - 1].snapshot().features()[feature],
        );
        result[FEATURE_COUNT + feature] = latest;
        result[2 * FEATURE_COUNT + feature] = latest - first;
    }
    result.map(|value| value.clamp(-10.0, 10.0))
}

fn probabilities(
    weights: &[f32],
    bias: [f32; CLASS_COUNT],
    x: &[f32; LINEAR_INPUT_COUNT],
) -> [f32; CLASS_COUNT] {
    let mut logits = bias;
    for class in 0..CLASS_COUNT {
        for input in 0..LINEAR_INPUT_COUNT {
            logits[class] += weights[class * LINEAR_INPUT_COUNT + input] * x[input];
        }
    }
    let max = logits.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let exp = logits.map(|value| (value - max).exp());
    let sum = exp.iter().sum::<f32>();
    exp.map(|value| value / sum)
}

fn deployed_class(values: [f32; CLASS_COUNT]) -> usize {
    if values[2] >= 0.90 && values[2] > values[0] && values[2] > values[1] {
        2
    } else if values[0] >= 0.90 && values[0] > values[1] && values[0] > values[2] {
        0
    } else {
        1
    }
}

fn class_index(direction: Direction) -> usize {
    match direction {
        Direction::Down => 0,
        Direction::Flat => 1,
        Direction::Up => 2,
    }
}

#[derive(Debug, Error)]
pub enum TrainingError {
    #[error("invalid training configuration")]
    InvalidConfig,
    #[error("{0} records are insufficient for leakage-free train and validation windows")]
    InsufficientRecords(usize),
    #[error("training split is missing at least one class: {0:?}")]
    MissingTrainingClass([usize; CLASS_COUNT]),
    #[error(transparent)]
    Standardizer(#[from] super::StandardizerError),
    #[error(transparent)]
    Model(#[from] super::LinearModelError),
}
