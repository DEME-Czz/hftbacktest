mod id;
mod market_data_stream;
mod orders;
mod protocol;
mod rest;
mod transport;
mod user_data_stream;

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};

use chrono::Utc;
use hftbacktest::{
    prelude::get_precision,
    types::{ErrorKind, LiveError, LiveEvent, Order, Status, Value},
};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::{broadcast, broadcast::Sender, mpsc::UnboundedSender};
use tokio_tungstenite::tungstenite;
use tracing::{debug, error, warn};

use crate::{
    exchange::binance_usdm::{
        id::{ClientOrderIdCodec, ClientOrderIdError},
        orders::{OrderManager, SharedOrderManager},
        rest::BinanceFuturesClient,
    },
    ports::{ExecutionVenue, MarketDataSource, PublishEvent, TradingInstrument},
};

use self::transport::{BackoffStrategy, ExponentialBackoff, Retry};

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ns() -> i64 {
    Utc::now().timestamp_micros().saturating_mul(1_000)
}

#[derive(Error, Debug)]
pub enum BinanceFuturesError {
    #[error("InstrumentNotFound")]
    InstrumentNotFound,
    #[error("InvalidRequest")]
    InvalidRequest,
    #[error("ListenKeyExpired")]
    ListenKeyExpired,
    #[error("ConnectionInterrupted")]
    ConnectionInterrupted,
    #[error("PublishSinkClosed")]
    PublishSinkClosed,
    #[error("ConnectionAbort: {0}")]
    ConnectionAbort(String),
    #[error("ReqError: {0:?}")]
    ReqError(#[from] reqwest::Error),
    #[error("OrderError: {code} - {msg}")]
    OrderError { code: i64, msg: String },
    #[error("PrefixUnmatched")]
    PrefixUnmatched,
    #[error("InvalidOrderPrefix")]
    InvalidOrderPrefix,
    #[error("MalformedClientOrderId")]
    MalformedClientOrderId,
    #[error("OrderRecoveryConflict")]
    OrderRecoveryConflict,
    #[error("UnsupportedPositionMode")]
    UnsupportedPositionMode,
    #[error("InvalidAccountState")]
    InvalidAccountState,
    #[error("OrderNotFound")]
    OrderNotFound,
    #[error("Tungstenite: {0:?}")]
    Tungstenite(#[from] tungstenite::Error),
}

impl From<BinanceFuturesError> for Value {
    fn from(value: BinanceFuturesError) -> Value {
        match value {
            BinanceFuturesError::ReqError(error) => {
                let mut map = HashMap::new();
                if let Some(code) = error.status() {
                    map.insert("status_code".to_string(), Value::String(code.to_string()));
                }
                map.insert("msg".to_string(), Value::String(error.to_string()));
                Value::Map(map)
            }
            BinanceFuturesError::OrderError { code, msg } => Value::Map({
                let mut map = HashMap::new();
                map.insert("code".to_string(), Value::Int(code));
                map.insert("msg".to_string(), Value::String(msg));
                map
            }),
            other => Value::String(other.to_string()),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct BinanceConfig {
    pub public_stream_url: String,
    #[serde(default)]
    pub private_stream_url: String,
    pub api_url: String,
    #[serde(default)]
    pub order_prefix: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub secret: String,
}

impl std::fmt::Debug for BinanceConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BinanceConfig")
            .field("public_stream_url", &self.public_stream_url)
            .field("private_stream_url", &self.private_stream_url)
            .field("api_url", &self.api_url)
            .field("order_prefix", &self.order_prefix)
            .field(
                "credentials_configured",
                &(!self.api_key.is_empty() && !self.secret.is_empty()),
            )
            .finish()
    }
}

type SharedSymbolSet = Arc<Mutex<HashSet<String>>>;

pub struct BinanceFutures {
    config: BinanceConfig,
    symbols: SharedSymbolSet,
    order_manager: SharedOrderManager,
    client: BinanceFuturesClient,
    symbol_tx: Sender<String>,
}

impl BinanceFutures {
    pub fn new(config: BinanceConfig) -> Result<Self, BinanceFuturesError> {
        let client_order_ids = ClientOrderIdCodec::new(&config.order_prefix)
            .map_err(|_| BinanceFuturesError::InvalidOrderPrefix)?;
        let order_manager = Arc::new(Mutex::new(OrderManager::new(client_order_ids)));
        let client = BinanceFuturesClient::new(&config.api_url, &config.api_key, &config.secret);
        let (symbol_tx, _) = broadcast::channel(500);

        Ok(Self {
            config,
            symbols: Default::default(),
            order_manager,
            client,
            symbol_tx,
        })
    }

    pub fn connect_market_data_stream(&mut self, ev_tx: UnboundedSender<PublishEvent>) {
        let base_url = self.config.public_stream_url.clone();
        let client = self.client.clone();
        let symbol_rx = self.symbol_tx.subscribe();
        let symbols = self.symbols.clone();

        // Construct the stream before spawning so register() always observes at least one
        // broadcast receiver. Reconnect reuses the same receiver and stream state.
        let mut stream =
            market_data_stream::MarketDataStream::new(client, ev_tx.clone(), symbols, symbol_rx);

        tokio::spawn(async move {
            let mut backoff = ExponentialBackoff::default();
            loop {
                debug!(%base_url, "connecting Binance public market stream");
                match stream.connect(&base_url).await {
                    Ok(()) => break,
                    Err(BinanceFuturesError::PublishSinkClosed) => {
                        debug!("market data consumer closed; stopping Binance public stream");
                        break;
                    }
                    Err(error) => {
                        error!(?error, "market data stream connection interrupted");
                        let _ = ev_tx.send(PublishEvent::LiveEvent(LiveEvent::Error(
                            LiveError::with(ErrorKind::ConnectionInterrupted, error.into()),
                        )));
                        tokio::time::sleep(backoff.backoff()).await;
                    }
                }
            }
        });
    }

    pub fn connect_user_data_stream(
        &self,
        instruments: Vec<TradingInstrument>,
        ev_tx: UnboundedSender<PublishEvent>,
    ) {
        let url_template = self.config.private_stream_url.clone();
        let client = self.client.clone();
        let order_manager = self.order_manager.clone();

        tokio::spawn(async move {
            let _ = Retry::new(ExponentialBackoff::default())
                .error_handler(|error: BinanceFuturesError| {
                    error!(?error, "user data stream connection interrupted");
                    if ev_tx.send(PublishEvent::AccountStreamDisconnected).is_err() {
                        return Err(error);
                    }
                    let _ = ev_tx.send(PublishEvent::LiveEvent(LiveEvent::Error(LiveError::with(
                        ErrorKind::ConnectionInterrupted,
                        error.into(),
                    ))));
                    Ok(())
                })
                .retry(|| async {
                    let mut stream = user_data_stream::UserDataStream::new(
                        client.clone(),
                        ev_tx.clone(),
                        order_manager.clone(),
                        instruments.clone(),
                    );
                    let listen_key = stream.get_listen_key().await?;
                    let url = url_template.replace("{listen_key}", &listen_key);
                    debug!("connecting Binance private user stream");
                    stream.connect(&url).await?;
                    Ok(())
                })
                .await;
        });
    }
}

pub(crate) fn validate_order_prefix(prefix: &str) -> Result<(), ClientOrderIdError> {
    ClientOrderIdCodec::new(prefix).map(|_| ())
}

impl MarketDataSource for BinanceFutures {
    fn register(&mut self, symbol: String) {
        let symbol = symbol.to_lowercase();
        let mut symbols = lock_recover(&self.symbols);
        if symbols.insert(symbol.clone()) {
            let _ = self.symbol_tx.send(symbol);
        }
    }

    fn start_market_data(&mut self, ev_tx: UnboundedSender<PublishEvent>) {
        self.connect_market_data_stream(ev_tx);
    }
}

impl ExecutionVenue for BinanceFutures {
    fn start_account_stream(
        &self,
        instruments: Vec<TradingInstrument>,
        ev_tx: UnboundedSender<PublishEvent>,
    ) {
        self.connect_user_data_stream(instruments, ev_tx);
    }

    fn open_orders(&self, symbol: &str) -> Vec<Order> {
        lock_recover(&self.order_manager).active_orders(symbol)
    }

    fn submit(&self, symbol: String, mut order: Order, tx: UnboundedSender<PublishEvent>) {
        let client = self.client.clone();
        let order_manager = self.order_manager.clone();
        tokio::spawn(async move {
            let client_order_id =
                lock_recover(&order_manager).prepare_client_order_id(symbol.clone(), order.clone());

            match client_order_id {
                Some(client_order_id) => {
                    let result = client
                        .submit_order(
                            &client_order_id,
                            &symbol,
                            order.side,
                            order.price_tick as f64 * order.tick_size,
                            get_precision(order.tick_size),
                            order.qty,
                            order.order_type,
                            order.time_in_force,
                        )
                        .await;
                    match result {
                        Ok(resp) => {
                            if let Some(order) = lock_recover(&order_manager)
                                .update_from_rest(&client_order_id, &resp)
                            {
                                let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                                    symbol,
                                    order,
                                }));
                            }
                        }
                        Err(error) => {
                            if let Some(order) = lock_recover(&order_manager)
                                .update_submit_fail(&client_order_id, &error)
                            {
                                let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                                    symbol: symbol.clone(),
                                    order,
                                }));
                            }
                            let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Error(
                                LiveError::with(ErrorKind::OrderError, error.into()),
                            )));
                        }
                    }
                }
                None => {
                    warn!(?order, "duplicate client order id; expiring local request");
                    order.req = Status::None;
                    order.status = Status::Expired;
                    let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order { symbol, order }));
                }
            }
        });
    }

    fn cancel(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>) {
        let client = self.client.clone();
        let order_manager = self.order_manager.clone();
        tokio::spawn(async move {
            let client_order_id =
                lock_recover(&order_manager).get_client_order_id(&symbol, order.order_id);

            if let Some(client_order_id) = client_order_id {
                match client.cancel_order(&client_order_id, &symbol).await {
                    Ok(resp) => {
                        if let Some(order) =
                            lock_recover(&order_manager).update_from_rest(&client_order_id, &resp)
                        {
                            let _ = tx
                                .send(PublishEvent::LiveEvent(LiveEvent::Order { symbol, order }));
                        }
                    }
                    Err(error) => {
                        if let Some(order) = lock_recover(&order_manager)
                            .update_cancel_fail(&client_order_id, &error)
                        {
                            let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                                symbol: symbol.clone(),
                                order,
                            }));
                        }
                        let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Error(
                            LiveError::with(ErrorKind::OrderError, error.into()),
                        )));
                    }
                }
            } else {
                warn!(order_id = order.order_id, "client order id not found");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hftbacktest::types::{LiveEvent, OrdType, Order, Side, Status, TimeInForce};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::mpsc::unbounded_channel,
        time,
    };

    use super::{BinanceConfig, BinanceFutures};
    use crate::ports::{ExecutionVenue, PublishEvent};

    async fn read_http_request(stream: &mut TcpStream) -> String {
        let mut request = Vec::new();
        let mut expected_len = None;
        loop {
            let mut chunk = [0_u8; 2048];
            let read = stream.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            if expected_len.is_none()
                && let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_len = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(str::trim)
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                expected_len = Some(header_end + 4 + content_len);
            }
            if expected_len.is_some_and(|len| request.len() >= len) {
                break;
            }
        }
        String::from_utf8(request).unwrap()
    }

    fn request_body(request: &str) -> &str {
        request.split_once("\r\n\r\n").map_or("", |(_, body)| body)
    }

    fn form_value<'a>(form: &'a str, key: &str) -> Option<&'a str> {
        form.split('&')
            .find_map(|part| part.split_once('=').filter(|(name, _)| *name == key))
            .map(|(_, value)| value)
    }

    #[tokio::test]
    async fn ambiguous_submit_queries_the_same_client_order_id_without_reposting() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            let mut client_order_id = None;
            for attempt in 0..2 {
                let accepted = time::timeout(Duration::from_secs(1), listener.accept()).await;
                let Ok(Ok((mut stream, _))) = accepted else {
                    break;
                };
                let request = read_http_request(&mut stream).await;
                if attempt == 0 {
                    client_order_id =
                        form_value(request_body(&request), "newClientOrderId").map(str::to_string);
                    requests.push(request);
                    // Simulate an order accepted by the exchange followed by a lost HTTP response.
                    drop(stream);
                    continue;
                }

                let id = client_order_id.as_deref().unwrap();
                let response_body = serde_json::json!({
                    "clientOrderId": id,
                    "cumQty": "0",
                    "cumQuote": "0",
                    "executedQty": "0",
                    "orderId": 123,
                    "avgPrice": "0",
                    "origQty": "1.00000",
                    "price": "100.0",
                    "reduceOnly": false,
                    "side": "BUY",
                    "positionSide": "BOTH",
                    "status": "NEW",
                    "stopPrice": "0",
                    "closePosition": false,
                    "symbol": "BTCUSDT",
                    "timeInForce": "GTC",
                    "type": "LIMIT",
                    "origType": "LIMIT",
                    "updateTime": 1234,
                    "workingType": "CONTRACT_PRICE",
                    "priceProtect": false,
                    "priceMatch": "NONE",
                    "selfTradePreventionMode": "NONE",
                    "goodTillDate": 0
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                requests.push(request);
            }
            (requests, client_order_id)
        });

        let connector = BinanceFutures::new(BinanceConfig {
            public_stream_url: "ws://127.0.0.1/".to_string(),
            private_stream_url: "ws://127.0.0.1/{listen_key}".to_string(),
            api_url: format!("http://{address}"),
            order_prefix: "strategy-a".to_string(),
            api_key: "key".to_string(),
            secret: "secret".to_string(),
        })
        .unwrap();
        let mut order = Order::new(
            42,
            1000,
            0.1,
            1.0,
            Side::Buy,
            OrdType::Limit,
            TimeInForce::GTC,
        );
        order.req = Status::New;
        let (tx, mut rx) = unbounded_channel();
        connector.submit("btcusdt".to_string(), order, tx);

        let recovered = loop {
            let event = time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap();
            if let PublishEvent::LiveEvent(LiveEvent::Order { order, .. }) = event {
                break order;
            }
        };
        let (requests, client_order_id) = server.await.unwrap();

        assert_eq!(requests.len(), 2, "submit must be followed by one query");
        assert!(requests[0].starts_with("POST /fapi/v1/order "));
        assert!(requests[1].starts_with("GET /fapi/v1/order?"));
        let client_order_id = client_order_id.unwrap();
        assert!(requests[1].contains(&format!("origClientOrderId={client_order_id}")));
        assert_eq!(recovered.order_id, 42);
        assert_eq!(recovered.status, Status::New);
        assert_eq!(recovered.req, Status::None);
    }
}
