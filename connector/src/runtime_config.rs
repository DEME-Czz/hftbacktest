use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct RuntimeConfig {
    pub name: String,
    pub connector: String,
}

impl RuntimeConfig {
    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text).context("failed to parse connector TOML")?;
        ensure!(!config.name.trim().is_empty(), "name must not be empty");
        ensure!(
            matches!(
                config.connector.as_str(),
                "binancefutures" | "binancespot" | "bybit"
            ),
            "unsupported connector: {}",
            config.connector
        );
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_launcher_parameters_from_exchange_config() {
        let config = RuntimeConfig::parse(
            r#"
name = "binancefutures-prod"
connector = "binancefutures"
api_url = "https://fapi.binance.com"
"#,
        )
        .unwrap();
        assert_eq!(config.name, "binancefutures-prod");
        assert_eq!(config.connector, "binancefutures");
    }

    #[test]
    fn rejects_unsupported_connector() {
        assert!(RuntimeConfig::parse("name='x'\nconnector='unknown'").is_err());
    }
}
