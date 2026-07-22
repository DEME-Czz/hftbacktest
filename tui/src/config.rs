use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuiConfig {
    pub connector_name: String,
    pub symbol: String,
    pub tick_size: f64,
    pub lot_size: f64,
    pub history_capacity: usize,
    pub poll_interval_ms: u64,
}

impl TuiConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read TUI config {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid TUI config {}", path.display()))
    }

    fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text).context("failed to parse TOML")?;
        ensure!(
            !config.connector_name.trim().is_empty(),
            "connector_name must not be empty"
        );
        ensure!(!config.symbol.trim().is_empty(), "symbol must not be empty");
        ensure!(
            config.tick_size.is_finite() && config.tick_size > 0.0,
            "tick_size must be finite and greater than zero"
        );
        ensure!(
            config.lot_size.is_finite() && config.lot_size > 0.0,
            "lot_size must be finite and greater than zero"
        );
        ensure!(
            config.history_capacity > 0,
            "history_capacity must be greater than zero"
        );
        ensure!(
            config.poll_interval_ms > 0,
            "poll_interval_ms must be greater than zero"
        );
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates_tui_config() {
        let config = TuiConfig::parse(
            r#"
connector_name = "binancefutures-prod"
symbol = "dogeusdt"
tick_size = 0.00001
lot_size = 1.0
history_capacity = 500
poll_interval_ms = 50
"#,
        )
        .unwrap();
        assert_eq!(config.symbol, "dogeusdt");
        assert_eq!(config.history_capacity, 500);
        assert_eq!(config.poll_interval_ms, 50);
    }

    #[test]
    fn rejects_non_positive_market_parameters() {
        let error = TuiConfig::parse(
            r#"
connector_name = "connector"
symbol = "dogeusdt"
tick_size = 0.0
lot_size = 1.0
history_capacity = 500
poll_interval_ms = 50
"#,
        );
        assert!(error.is_err());
    }
}
