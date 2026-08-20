use std::{
    fmt::{self, Debug, Write},
    future::Future,
    marker::PhantomData,
    time::{Duration, Instant},
};

use chrono::Utc;
use hashbrown::Equivalent;
use hftbacktest::prelude::OrderId;
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{
    Deserialize, Deserializer,
    de::{self, Error, Visitor},
};
use sha2::Sha256;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, client_async_tls_with_config, connect_async,
    tungstenite::{self, client::IntoClientRequest, handshake::client::Response},
};

pub async fn connect_websocket(
    request: impl IntoClientRequest,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response), tungstenite::Error> {
    let request = request.into_client_request()?;
    let Some(proxy) = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .ok()
    else {
        return connect_async(request).await;
    };

    let proxy = reqwest::Url::parse(&proxy).map_err(|error| {
        tungstenite::Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
    })?;
    let proxy_host = proxy.host_str().ok_or_else(|| {
        tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "HTTPS_PROXY has no host",
        ))
    })?;
    let proxy_port = proxy.port_or_known_default().ok_or_else(|| {
        tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "HTTPS_PROXY has no port",
        ))
    })?;
    let target_host = request.uri().host().ok_or_else(|| {
        tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "WebSocket URL has no host",
        ))
    })?;
    let target_port = request.uri().port_u16().unwrap_or(443);

    let mut stream = TcpStream::connect((proxy_host, proxy_port)).await?;
    let authority = format!("{target_host}:{target_port}");
    stream
        .write_all(
            format!(
                "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;

    let mut response = Vec::with_capacity(512);
    let mut byte = [0_u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() >= 8192 {
            return Err(tungstenite::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP proxy response headers are too large",
            )));
        }
        if stream.read(&mut byte).await? == 0 {
            return Err(tungstenite::Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "HTTP proxy closed the CONNECT tunnel",
            )));
        }
        response.push(byte[0]);
    }
    if !(response.starts_with(b"HTTP/1.1 200") || response.starts_with(b"HTTP/1.0 200")) {
        return Err(tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            String::from_utf8_lossy(&response).into_owned(),
        )));
    }

    client_async_tls_with_config(request, stream, None, None).await
}

struct F64Visitor;
impl Visitor<'_> for F64Visitor {
    type Value = Option<f64>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string containing an f64 number")
    }
    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where E: de::Error {
        if s.is_empty() { Ok(None) } else { Ok(Some(s.parse::<f64>().map_err(Error::custom)?)) }
    }
}

struct OptionF64Visitor;
impl<'de> Visitor<'de> for OptionF64Visitor {
    type Value = Option<f64>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an optional string containing an f64 number")
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> where E: Error { Ok(None) }
    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where D: Deserializer<'de> {
        deserializer.deserialize_str(F64Visitor)
    }
}

pub fn from_str_to_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where D: Deserializer<'de> {
    deserializer.deserialize_str(F64Visitor).map(|value| value.unwrap_or(0.0))
}

pub fn from_str_to_f64_opt<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where D: Deserializer<'de> {
    deserializer.deserialize_option(OptionF64Visitor)
}

pub fn to_lowercase<'de, D>(deserializer: D) -> Result<String, D::Error>
where D: Deserializer<'de> {
    let s: &str = Deserialize::deserialize(deserializer)?;
    Ok(s.to_lowercase())
}

pub fn sign_hmac_sha256(secret: &str, s: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(s.as_bytes());
    let hash = mac.finalize().into_bytes();
    let mut out = String::with_capacity(hash.len() * 2);
    for byte in hash {
        write!(&mut out, "{byte:02x}").unwrap();
    }
    out
}

pub type PxQty = (f64, f64);
pub fn parse_px_qty_tup(px: String, qty: String) -> Result<PxQty, std::num::ParseFloatError> {
    Ok((px.parse()?, qty.parse()?))
}

pub fn parse_depth(
    bids: Vec<(String, String)>,
    asks: Vec<(String, String)>,
) -> Result<(Vec<PxQty>, Vec<PxQty>), std::num::ParseFloatError> {
    let bids = bids
        .into_iter()
        .map(|(px, qty)| parse_px_qty_tup(px, qty))
        .collect::<Result<Vec<_>, _>>()?;
    let asks = asks
        .into_iter()
        .map(|(px, qty)| parse_px_qty_tup(px, qty))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((bids, asks))
}

pub trait BackoffStrategy { fn backoff(&mut self) -> Duration; }

pub struct ExponentialBackoff {
    last_attempt: Instant,
    factor: u32,
    last_delay: Option<Duration>,
    reset_interval: Duration,
    min_delay: Duration,
    max_delay: Duration,
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self {
            last_attempt: Instant::now(),
            factor: 2,
            last_delay: None,
            reset_interval: Duration::from_secs(300),
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(60),
        }
    }
}

impl BackoffStrategy for ExponentialBackoff {
    fn backoff(&mut self) -> Duration {
        if self.last_attempt.elapsed() > self.reset_interval { self.last_delay = None; }
        self.last_attempt = Instant::now();
        let delay = self.last_delay
            .map(|last| last.saturating_mul(self.factor).min(self.max_delay))
            .unwrap_or(self.min_delay);
        self.last_delay = Some(delay);
        delay
    }
}

pub struct Retry<O, E, Backoff, ErrorHandler> {
    backoff: Backoff,
    error_handler: Option<ErrorHandler>,
    _marker: PhantomData<(O, E)>,
}

impl<O, E, Backoff, ErrorHandler> Retry<O, E, Backoff, ErrorHandler>
where
    E: Debug,
    Backoff: BackoffStrategy,
    ErrorHandler: FnMut(E) -> Result<(), E>,
{
    pub fn new(backoff: Backoff) -> Self {
        Self { backoff, error_handler: None, _marker: PhantomData }
    }
    pub fn error_handler(self, error_handler: ErrorHandler) -> Self {
        Self { error_handler: Some(error_handler), ..self }
    }
    pub async fn retry<F, Fut>(&mut self, func: F) -> Result<O, E>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<O, E>>,
    {
        loop {
            match func().await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    if let Some(handler) = self.error_handler.as_mut() { handler(error)?; }
                    tokio::time::sleep(self.backoff.backoff()).await;
                }
            }
        }
    }
}

#[derive(Eq, Hash, PartialEq, Debug)]
pub struct SymbolOrderId { pub symbol: String, pub order_id: OrderId }
impl SymbolOrderId {
    pub fn new(symbol: String, order_id: OrderId) -> Self { Self { symbol, order_id } }
}

#[derive(Eq, Hash, PartialEq, Debug)]
pub struct RefSymbolOrderId<'a> { pub symbol: &'a str, pub order_id: OrderId }
impl<'a> RefSymbolOrderId<'a> {
    pub fn new(symbol: &'a str, order_id: OrderId) -> Self { Self { symbol, order_id } }
}
impl Equivalent<SymbolOrderId> for RefSymbolOrderId<'_> {
    fn equivalent(&self, key: &SymbolOrderId) -> bool {
        key.symbol == self.symbol && key.order_id == self.order_id
    }
}

pub fn generate_rand_string(length: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect()
}

pub fn get_timestamp() -> u64 { Utc::now().timestamp_millis() as u64 }
