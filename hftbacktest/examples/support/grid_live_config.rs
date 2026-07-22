use anyhow::{Context, Result, ensure};
use hftbacktest::alpha::AlphaConfig;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridLiveConfig {
    pub connector_name: String,
    pub symbol: String,
    pub tick_size: f64,
    pub lot_size: f64,
    pub startup_timeout_seconds: u64,
    pub dataset_path: Option<PathBuf>,
    pub model_path: Option<PathBuf>,
    pub relative_half_spread: f64,
    pub relative_grid_interval: f64,
    pub grid_num: usize,
    pub min_grid_step: f64,
    pub skew: f64,
    pub order_qty: f64,
    pub max_position: f64,
    pub alpha_confidence_threshold: f32,
    pub alpha_calibrated_return: f64,
    pub alpha_max_relative_offset: f64,
    pub alpha_smoothing: f64,
}

impl GridLiveConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        ensure!(
            !config.connector_name.trim().is_empty(),
            "connector_name must not be empty"
        );
        ensure!(!config.symbol.trim().is_empty(), "symbol must not be empty");
        for (name, value) in [
            ("tick_size", config.tick_size),
            ("lot_size", config.lot_size),
            ("relative_half_spread", config.relative_half_spread),
            ("relative_grid_interval", config.relative_grid_interval),
            ("min_grid_step", config.min_grid_step),
            ("skew", config.skew),
            ("order_qty", config.order_qty),
            ("max_position", config.max_position),
        ] {
            ensure!(
                value.is_finite() && value > 0.0,
                "{name} must be finite and greater than zero"
            );
        }
        ensure!(
            config.startup_timeout_seconds > 0,
            "startup_timeout_seconds must be greater than zero"
        );
        ensure!(config.grid_num > 0, "grid_num must be greater than zero");
        ensure!(
            (0.0..=1.0).contains(&config.alpha_confidence_threshold),
            "alpha_confidence_threshold must be between zero and one"
        );
        ensure!(
            config.alpha_calibrated_return.is_finite() && config.alpha_calibrated_return >= 0.0,
            "alpha_calibrated_return must be finite and non-negative"
        );
        ensure!(
            config.alpha_max_relative_offset.is_finite() && config.alpha_max_relative_offset >= 0.0,
            "alpha_max_relative_offset must be finite and non-negative"
        );
        ensure!(
            config.alpha_smoothing.is_finite() && (0.0..=1.0).contains(&config.alpha_smoothing),
            "alpha_smoothing must be between zero and one"
        );
        Ok(config)
    }

    pub fn alpha_config(&self) -> AlphaConfig {
        AlphaConfig {
            confidence_threshold: self.alpha_confidence_threshold,
            calibrated_return: self.alpha_calibrated_return,
            max_relative_offset: self.alpha_max_relative_offset,
            smoothing: self.alpha_smoothing,
        }
    }
}
