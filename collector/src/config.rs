use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub enum Exchange {
    #[serde(rename = "binance", alias = "binancespot")]
    BinanceSpot,
    #[serde(rename = "binancefutures", alias = "binancefuturesum")]
    BinanceFuturesUm,
    #[serde(rename = "binancefuturescm")]
    BinanceFuturesCm,
    #[serde(rename = "bybit")]
    Bybit,
    #[serde(rename = "hyperliquid")]
    Hyperliquid,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectorConfig {
    pub output_path: String,
    pub exchange: Exchange,
    pub symbols: Vec<String>,
    pub streams: Vec<String>,
    #[serde(default)]
    pub proxy: Option<String>,
}

impl CollectorConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read collector config {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid collector config {}", path.display()))
    }

    fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text).context("failed to parse TOML")?;
        ensure!(
            !config.output_path.trim().is_empty(),
            "output_path must not be empty"
        );
        ensure!(!config.symbols.is_empty(), "symbols must not be empty");
        ensure!(
            config
                .symbols
                .iter()
                .all(|symbol| !symbol.trim().is_empty()),
            "symbols must not contain an empty value"
        );
        ensure!(!config.streams.is_empty(), "streams must not be empty");
        ensure!(
            config
                .streams
                .iter()
                .all(|stream| !stream.trim().is_empty()),
            "streams must not contain an empty value"
        );
        ensure!(
            config
                .proxy
                .as_ref()
                .is_none_or(|proxy| !proxy.trim().is_empty()),
            "proxy must not be empty"
        );
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
output_path = "data/market"
exchange = "binancefutures"
symbols = ["dogeusdt", "btcusdt"]
streams = ["$symbol@trade", "$symbol@bookTicker", "$symbol@depth@100ms"]
proxy = "127.0.0.1:7890"
"#;

    #[test]
    fn parses_all_collection_parameters_from_toml() {
        let config = CollectorConfig::parse(VALID).unwrap();

        assert_eq!(config.output_path, "data/market");
        assert_eq!(config.exchange, Exchange::BinanceFuturesUm);
        assert_eq!(config.symbols, ["dogeusdt", "btcusdt"]);
        assert_eq!(config.streams.len(), 3);
        assert_eq!(config.proxy.as_deref(), Some("127.0.0.1:7890"));
    }

    #[test]
    fn rejects_unknown_fields() {
        let error = CollectorConfig::parse(&format!("{VALID}\nmisspelled = true")).unwrap_err();
        assert!(format!("{error:#}").contains("unknown field"));
    }

    #[test]
    fn rejects_empty_symbols_and_streams() {
        let no_symbols = VALID.replace("symbols = [\"dogeusdt\", \"btcusdt\"]", "symbols = []");
        assert!(CollectorConfig::parse(&no_symbols).is_err());

        let no_streams = VALID.replace(
            "streams = [\"$symbol@trade\", \"$symbol@bookTicker\", \"$symbol@depth@100ms\"]",
            "streams = []",
        );
        assert!(CollectorConfig::parse(&no_streams).is_err());
    }

    #[test]
    fn rejects_empty_proxy() {
        let config = VALID.replace("127.0.0.1:7890", "  ");
        let error = CollectorConfig::parse(&config).unwrap_err();
        assert!(format!("{error:#}").contains("proxy must not be empty"));
    }
}
