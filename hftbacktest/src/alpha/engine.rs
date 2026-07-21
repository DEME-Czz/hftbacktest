use thiserror::Error;

use crate::depth::MarketDepth;

use super::{
    AlphaModel, AlphaPrediction, Direction, LobSnapshot, LobWindow, PredictionError, SnapshotError,
};

#[derive(Clone, Copy, Debug)]
pub struct AlphaConfig {
    pub confidence_threshold: f32,
    pub calibrated_return: f64,
    pub max_relative_offset: f64,
    pub smoothing: f64,
}

impl Default for AlphaConfig {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.60,
            calibrated_return: 0.0,
            max_relative_offset: 0.0,
            smoothing: 0.25,
        }
    }
}

impl AlphaConfig {
    fn validate(self) -> Result<Self, AlphaEngineError<std::convert::Infallible>> {
        if !(0.0..=1.0).contains(&self.confidence_threshold)
            || !self.calibrated_return.is_finite()
            || self.calibrated_return < 0.0
            || !self.max_relative_offset.is_finite()
            || self.max_relative_offset < 0.0
            || !self.smoothing.is_finite()
            || !(0.0..=1.0).contains(&self.smoothing)
        {
            return Err(AlphaEngineError::InvalidConfig);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AlphaSignal {
    pub direction: Direction,
    pub prediction: Option<AlphaPrediction>,
    pub price_offset: f64,
}

impl Default for AlphaSignal {
    fn default() -> Self {
        Self {
            direction: Direction::Flat,
            prediction: None,
            price_offset: 0.0,
        }
    }
}

pub struct AlphaEngine<M> {
    model: M,
    config: AlphaConfig,
    window: LobWindow,
    latest_signal: AlphaSignal,
}

impl<M: AlphaModel> AlphaEngine<M> {
    pub fn new(model: M, config: AlphaConfig) -> Result<Self, AlphaEngineError<M::Error>> {
        let config = config
            .validate()
            .map_err(|_| AlphaEngineError::InvalidConfig)?;
        Ok(Self {
            model,
            config,
            window: LobWindow::new(),
            latest_signal: AlphaSignal::default(),
        })
    }

    pub fn update(
        &mut self,
        depth: &impl MarketDepth,
    ) -> Result<AlphaSignal, AlphaEngineError<M::Error>> {
        let snapshot = LobSnapshot::from_depth(depth)?;
        if !self.window.push(snapshot) || !self.window.is_ready() {
            return Ok(self.latest_signal);
        }

        let prediction = self
            .model
            .predict(&self.window)
            .map_err(AlphaEngineError::Model)?;
        let direction = prediction.direction(self.config.confidence_threshold);
        let target_offset = if direction == Direction::Flat {
            0.0
        } else {
            let mid_price = (depth.best_bid() + depth.best_ask()) / 2.0;
            let relative_offset = (prediction.directional_score() * self.config.calibrated_return)
                .clamp(
                    -self.config.max_relative_offset,
                    self.config.max_relative_offset,
                );
            mid_price * relative_offset
        };
        let price_offset = self.config.smoothing * target_offset
            + (1.0 - self.config.smoothing) * self.latest_signal.price_offset;
        self.latest_signal = AlphaSignal {
            direction,
            prediction: Some(prediction),
            price_offset,
        };
        Ok(self.latest_signal)
    }

    pub fn latest_signal(&self) -> AlphaSignal {
        self.latest_signal
    }

    pub fn reset(&mut self) {
        self.window.clear();
        self.latest_signal = AlphaSignal::default();
    }

    pub fn window(&self) -> &LobWindow {
        &self.window
    }
}

#[derive(Debug, Error)]
pub enum AlphaEngineError<E: std::error::Error + 'static> {
    #[error("invalid alpha configuration")]
    InvalidConfig,
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Prediction(#[from] PredictionError),
    #[error("alpha model failed: {0}")]
    Model(E),
}
