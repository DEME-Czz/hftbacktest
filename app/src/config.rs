use std::net::IpAddr;

use serde::Deserialize;
use thiserror::Error;

use crate::{
    exchange::binance_usdm::{BinanceConfig, validate_order_prefix},
    live::config::RuntimeConfig,
    ports::RunMode,
};

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
    #[error("order_prefix must contain 1-12 Binance-safe characters")]
    InvalidOrderPrefix,
    #[error("invalid live safety configuration")]
    InvalidSafety,
    #[error("execute mode requires a matched Binance endpoint environment")]
    UntrustedExecutionEndpoint,
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
        self.runtime
            .safety
            .validate()
            .map_err(|_| ConfigError::InvalidSafety)?;
        validate_order_prefix(&self.exchange.order_prefix)
            .map_err(|_| ConfigError::InvalidOrderPrefix)?;
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
            if !matched_execution_environment(
                &self.exchange.api_url,
                &self.exchange.public_stream_url,
                &private_url,
                self.exchange.allow_test_endpoints,
            ) {
                return Err(ConfigError::UntrustedExecutionEndpoint);
            }
        }
        Ok(())
    }
}

fn matched_execution_environment(
    api: &str,
    public_stream: &str,
    private_stream: &str,
    allow_test_endpoints: bool,
) -> bool {
    let (Ok(api), Ok(public_stream), Ok(private_stream)) = (
        reqwest::Url::parse(api),
        reqwest::Url::parse(public_stream),
        reqwest::Url::parse(private_stream),
    ) else {
        return false;
    };
    if allow_test_endpoints
        && is_loopback(&api)
        && is_loopback(&public_stream)
        && is_loopback(&private_stream)
        && api.scheme() == "http"
        && public_stream.scheme() == "ws"
        && private_stream.scheme() == "ws"
    {
        return true;
    }

    matches_execution_profile(
        &api,
        &public_stream,
        &private_stream,
        "fapi.binance.com",
        "fstream.binance.com",
        "/public/stream",
        "/private/ws/configured-listen-key",
    ) || matches_execution_profile(
        &api,
        &public_stream,
        &private_stream,
        "demo-fapi.binance.com",
        "demo-fstream.binance.com",
        "/ws",
        "/ws/configured-listen-key",
    ) || matches_execution_profile(
        &api,
        &public_stream,
        &private_stream,
        "testnet.binancefuture.com",
        "stream.binancefuture.com",
        "/ws",
        "/ws/configured-listen-key",
    )
}

fn matches_execution_profile(
    api: &reqwest::Url,
    public_stream: &reqwest::Url,
    private_stream: &reqwest::Url,
    api_host: &str,
    stream_host: &str,
    public_stream_path: &str,
    private_stream_path: &str,
) -> bool {
    matches_endpoint(api, "https", api_host, "/")
        && matches_endpoint(public_stream, "wss", stream_host, public_stream_path)
        && matches_endpoint(private_stream, "wss", stream_host, private_stream_path)
}

fn matches_endpoint(url: &reqwest::Url, scheme: &str, host: &str, path: &str) -> bool {
    url.scheme() == scheme
        && url.host_str() == Some(host)
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.path() == path
        && url.query().is_none()
        && url.fragment().is_none()
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
