use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainAlphaConfig {
    pub input: PathBuf,
    pub output: PathBuf,
    pub horizon: usize,
    pub threshold: f64,
    pub train_ratio: f64,
    pub epochs: usize,
    pub learning_rate: f32,
    pub l2: f32,
}

impl TrainAlphaConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        ensure!(config.horizon > 0, "horizon must be greater than zero");
        ensure!(
            config.threshold.is_finite() && config.threshold > 0.0,
            "threshold must be finite and greater than zero"
        );
        ensure!(
            config.train_ratio > 0.0 && config.train_ratio < 1.0,
            "train_ratio must be between zero and one"
        );
        ensure!(config.epochs > 0, "epochs must be greater than zero");
        ensure!(
            config.learning_rate.is_finite() && config.learning_rate > 0.0,
            "learning_rate must be finite and greater than zero"
        );
        ensure!(
            config.l2.is_finite() && config.l2 >= 0.0,
            "l2 must be finite and non-negative"
        );
        Ok(config)
    }
}
