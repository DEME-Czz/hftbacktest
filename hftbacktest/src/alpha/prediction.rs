use thiserror::Error;

const PROBABILITY_SUM_TOLERANCE: f32 = 1e-4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Down,
    Flat,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlphaPrediction {
    down: f32,
    flat: f32,
    up: f32,
}

impl AlphaPrediction {
    pub fn new(down: f32, flat: f32, up: f32) -> Result<Self, PredictionError> {
        let probabilities = [down, flat, up];
        if probabilities
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(PredictionError::InvalidProbability);
        }
        let sum = down + flat + up;
        if (sum - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
            return Err(PredictionError::InvalidSum(sum));
        }
        Ok(Self { down, flat, up })
    }

    pub fn down(self) -> f32 {
        self.down
    }

    pub fn flat(self) -> f32 {
        self.flat
    }

    pub fn up(self) -> f32 {
        self.up
    }

    pub fn direction(self, confidence_threshold: f32) -> Direction {
        if self.up >= confidence_threshold && self.up > self.down && self.up > self.flat {
            Direction::Up
        } else if self.down >= confidence_threshold && self.down > self.up && self.down > self.flat
        {
            Direction::Down
        } else {
            Direction::Flat
        }
    }

    pub fn directional_score(self) -> f64 {
        f64::from(self.up - self.down)
    }
}

#[derive(Debug, Error)]
pub enum PredictionError {
    #[error("probabilities must be finite values between zero and one")]
    InvalidProbability,
    #[error("class probabilities must sum to one, found {0}")]
    InvalidSum(f32),
}
