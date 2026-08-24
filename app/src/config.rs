use std::net::IpAddr;

use serde::Deserialize;
use thiserror::Error;

use crate::{exchange::binance_usdm::BinanceConfig, live::config::RuntimeConfig, ports::RunMode};

#[derive(Clone, Debug, Deserialize)]
pub struct AppConfig {
    #[serde(flatten)]
    pub exchange: BinanceConfig,
    #[serde(flatten)]
    pub runtime: RuntimeConfig,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ConfigError {
    #[error("invalid TOML configuration")]
    InvalidToml,
    #[error("public_stream_url and api_url are required")]
    MissingPublicEndpoint,
    #[error("remote Binance endpoints must use wss/https")]
    InsecureEndpoint,
    #[error("api_key and secret must be configured together")]
    PartialCredentials,
    #[error("execute mode requires API credentials")]
    MissingCredentials,
    #[error("execute mode requires a secure private_stream_url with {{listen_key}}")]
    InvalidPrivateStream,
    #[error("risk limits must be finite and positive")]
    InvalidRisk,
}

impl AppConfig {
    pub fn parse_and_validate(raw: &str, mode: RunMode) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(raw).map_err(|_| ConfigError::InvalidToml)?;
        config.validate(mode)?;
        Ok(config)
    }

    pub fn validate(&self, mode: RunMode) -> Result<(), ConfigError> {
        self.runtime
            .risk
            .validate()
            .map_err(|_| ConfigError::InvalidRisk)?;
        if self.exchange.public_stream_url.trim().is_empty()
            || self.exchange.api_url.trim().is_empty()
        {
            return Err(ConfigError::MissingPublicEndpoint);
        }
        if !valid_transport(&self.exchange.public_stream_url, "wss", "ws")
            || !valid_transport(&self.exchange.api_url, "https", "http")
        {
            return Err(ConfigError::InsecureEndpoint);
        }

        let has_key = !self.exchange.api_key.trim().is_empty();
        let has_secret = !self.exchange.secret.trim().is_empty();
        if has_key != has_secret {
            return Err(ConfigError::PartialCredentials);
        }
        if mode.allows_trading() && !has_key {
            return Err(ConfigError::MissingCredentials);
        }
        if mode.allows_trading() {
            let private_url = self
                .exchange
                .private_stream_url
                .replace("{listen_key}", "configured-listen-key");
            if !self.exchange.private_stream_url.contains("{listen_key}")
                || !valid_transport(&private_url, "wss", "ws")
            {
                return Err(ConfigError::InvalidPrivateStream);
            }
        }
        Ok(())
    }
}

fn valid_transport(raw: &str, secure_scheme: &str, loopback_scheme: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if url.host_str().is_none() {
        return false;
    }
    if url.scheme() == secure_scheme {
        return true;
    }
    url.scheme() == loopback_scheme && is_loopback(&url)
}

fn is_loopback(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}
