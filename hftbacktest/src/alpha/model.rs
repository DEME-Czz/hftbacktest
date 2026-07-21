use std::{convert::Infallible, path::Path};

use thiserror::Error;

use super::{AlphaPrediction, LinearAlphaModel, LinearModelError, LobWindow};

pub trait AlphaModel {
    type Error: std::error::Error + Send + Sync + 'static;

    fn predict(&mut self, input: &LobWindow) -> Result<AlphaPrediction, Self::Error>;
}

/// Safe fallback used until a trained model backend is configured.
#[derive(Default)]
pub struct FlatAlphaModel;

impl AlphaModel for FlatAlphaModel {
    type Error = Infallible;

    fn predict(&mut self, _input: &LobWindow) -> Result<AlphaPrediction, Self::Error> {
        Ok(AlphaPrediction::new(0.0, 1.0, 0.0).expect("flat probabilities are valid"))
    }
}

pub enum RuntimeAlphaModel {
    Flat(FlatAlphaModel),
    Linear(LinearAlphaModel),
}

impl RuntimeAlphaModel {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RuntimeModelError> {
        let json = std::fs::read_to_string(path)?;
        Ok(Self::Linear(LinearAlphaModel::from_json(&json)?))
    }

    pub fn flat() -> Self {
        Self::Flat(FlatAlphaModel)
    }

    pub fn is_trained(&self) -> bool {
        matches!(self, Self::Linear(_))
    }
}

impl AlphaModel for RuntimeAlphaModel {
    type Error = RuntimeModelError;

    fn predict(&mut self, input: &LobWindow) -> Result<AlphaPrediction, Self::Error> {
        match self {
            Self::Flat(model) => Ok(model.predict(input).expect("flat inference is infallible")),
            Self::Linear(model) => Ok(model.predict(input)?),
        }
    }
}

#[derive(Debug, Error)]
pub enum RuntimeModelError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Linear(#[from] LinearModelError),
}
