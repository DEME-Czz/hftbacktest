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
use hftbacktest::types::{ErrorKind, LiveError, LiveEvent, Order, Status, Value};
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
    #[error("DepthBufferOverflow")]
    DepthBufferOverflow,
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

impl BinanceFuturesError {
    fn submission_is_ambiguous(&self) -> bool {
        match self {
            Self::ReqError(_) => true,
            Self::OrderError { code, .. } => {
                !matches!(*code, -5022 | -2027 | -2019 | -1015 | -1008)
            }
            _ => false,
        }
    }

    fn submission_capacity_rejection_code(&self) -> Option<i64> {
        match self {
            Self::OrderError { code: -2027, .. } => Some(-2027),
            _ => None,
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
    #[serde(default)]
    pub allow_test_endpoints: bool,
}

impl std::fmt::Debug for BinanceConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BinanceConfig")
            .field("public_stream_url", &self.public_stream_url)
            .field("private_stream_url", &self.private_stream_url)
            .field("api_url", &self.api_url)
            .field("order_prefix", &self.order_prefix)
            .field("allow_test_endpoints", &self.allow_test_endpoints)
            .field(
                "credentials_configured",
                &(!self.api_key.is_empty() && !self.secret.is_empty()),
            )
            .finish()
    }
}

type SharedSymbolSet = Arc<Mutex<HashSet<String>>>;
type SharedInFlightSubmissions = Arc<Mutex<HashSet<String>>>;

struct InFlightSubmissionGuard {
    client_order_id: String,
    submissions: SharedInFlightSubmissions,
}

impl InFlightSubmissionGuard {
    fn new(client_order_id: String, submissions: SharedInFlightSubmissions) -> Self {
        Self {
            client_order_id,
            submissions,
        }
    }
}

impl Drop for InFlightSubmissionGuard {
    fn drop(&mut self) {
        lock_recover(&self.submissions).remove(&self.client_order_id);
    }
}

pub struct BinanceFutures {
    config: BinanceConfig,
    symbols: SharedSymbolSet,
    order_manager: SharedOrderManager,
    submissions_in_flight: SharedInFlightSubmissions,
    cancellations_in_flight: SharedInFlightSubmissions,
    client: BinanceFuturesClient,
    symbol_tx: Sender<String>,
}

impl BinanceFutures {
    pub fn new(config: BinanceConfig) -> Result<Self, BinanceFuturesError> {
        let client_order_ids = ClientOrderIdCodec::new(&config.order_prefix)
            .map_err(|_| BinanceFuturesError::InvalidOrderPrefix)?;
        let order_manager = Arc::new(Mutex::new(OrderManager::new(client_order_ids)));
        let client = BinanceFuturesClient::new(&config.api_url, &config.api_key, &config.secret)?;
        let (symbol_tx, _) = broadcast::channel(500);

        Ok(Self {
            config,
            symbols: Default::default(),
            order_manager,
            submissions_in_flight: Default::default(),
            cancellations_in_flight: Default::default(),
            client,
            symbol_tx,
        })
    }

    pub fn connect_market_data_stream(&mut self, ev_tx: UnboundedSender<PublishEvent>) {
        let base_url = self.config.public_stream_url.clone();
        let client = self.client.clone();
        let symbol_rx = self.symbol_tx.subscribe();
        let symbols = self.symbols.clone();

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
                        if ev_tx.send(PublishEvent::MarketStreamDisconnected).is_err() {
                            break;
                        }
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

    fn submit(
        &self,
        symbol: String,
        mut order: Order,
        lot_size: f64,
        tx: UnboundedSender<PublishEvent>,
    ) {
        let client = self.client.clone();
        let order_manager = self.order_manager.clone();
        let Some(client_order_id) =
            lock_recover(&order_manager).prepare_client_order_id(symbol.clone(), order.clone())
        else {
            warn!(?order, "duplicate client order id; expiring local request");
            order.req = Status::None;
            order.status = Status::Expired;
            let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order { symbol, order }));
            return;
        };
        let submissions_in_flight = self.submissions_in_flight.clone();
        lock_recover(&submissions_in_flight).insert(client_order_id.clone());

        tokio::spawn(async move {
            let _submission_guard =
                InFlightSubmissionGuard::new(client_order_id.clone(), submissions_in_flight);
            let result = client
                .submit_order(
                    &client_order_id,
                    &symbol,
                    order.side,
                    order.price_tick as f64 * order.tick_size,
                    order.tick_size,
                    order.qty,
                    lot_size,
                    order.order_type,
                    order.time_in_force,
                )
                .await;
            match result {
                Ok(resp) => {
                    if let Some(order) =
                        lock_recover(&order_manager).update_from_rest(&client_order_id, &resp)
                    {
                        let _ =
                            tx.send(PublishEvent::LiveEvent(LiveEvent::Order { symbol, order }));
                    }
                }
                Err(error) => {
                    if error.submission_is_ambiguous() {
                        let mut confirmed = None;
                        for delay_ms in [50_u64, 150, 300] {
                            match client.query_order(&client_order_id, &symbol).await {
                                Ok(Some(response)) => {
                                    confirmed = Some(response);
                                    break;
                                }
                                Ok(None) | Err(_) => {
                                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms))
                                        .await;
                                }
                            }
                        }
                        if let Some(response) = confirmed {
                            if let Some(order) = lock_recover(&order_manager)
                                .update_from_rest(&client_order_id, &response)
                            {
                                let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                                    symbol,
                                    order,
                                }));
                            }
                            return;
                        }
                        error!(
                            %symbol,
                            %client_order_id,
                            "order submission outcome is unresolved; execution latched off"
                        );
                        let _ = tx.send(PublishEvent::ExecutionUncertain {
                            symbol: symbol.clone(),
                        });
                    } else {
                        if let Some(code) = error.submission_capacity_rejection_code() {
                            // Publish this before the local terminal order update. The service must
                            // block the rejected side before that update marks quotes dirty and
                            // could otherwise immediately resubmit the same impossible order.
                            let _ = tx.send(PublishEvent::SubmissionCapacityRejected {
                                symbol: symbol.clone(),
                                side: order.side,
                                code,
                            });
                        }
                        if let Some(order) = lock_recover(&order_manager)
                            .update_submit_fail(&client_order_id, &error)
                        {
                            let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                                symbol: symbol.clone(),
                                order,
                            }));
                        }
                    }
                    let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Error(LiveError::with(
                        ErrorKind::OrderError,
                        error.into(),
                    ))));
                }
            }
        });
    }

    fn cancel(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>) {
        let client = self.client.clone();
        let order_manager = self.order_manager.clone();
        let submissions_in_flight = self.submissions_in_flight.clone();
        let Some(client_order_id) =
            lock_recover(&order_manager).get_client_order_id(&symbol, order.order_id)
        else {
            warn!(order_id = order.order_id, "client order id not found");
            return;
        };
        let cancellations_in_flight = self.cancellations_in_flight.clone();
        if !lock_recover(&cancellations_in_flight).insert(client_order_id.clone()) {
            return;
        }
        tokio::spawn(async move {
            let _cancellation_guard =
                InFlightSubmissionGuard::new(client_order_id.clone(), cancellations_in_flight);
            while lock_recover(&submissions_in_flight).contains(&client_order_id) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let still_tracked = lock_recover(&order_manager)
                .get_client_order_id(&symbol, order.order_id)
                .is_some_and(|current| current == client_order_id);
            if !still_tracked {
                return;
            }

            match client.cancel_order(&client_order_id, &symbol).await {
                Ok(resp) => {
                    if let Some(order) =
                        lock_recover(&order_manager).update_from_rest(&client_order_id, &resp)
                    {
                        let _ =
                            tx.send(PublishEvent::LiveEvent(LiveEvent::Order { symbol, order }));
                    }
                }
                Err(error) => {
                    if matches!(error, BinanceFuturesError::OrderError { code: -2011, .. }) {
                        match client.query_order(&client_order_id, &symbol).await {
                            Ok(Some(response)) => {
                                if let Some(order) = lock_recover(&order_manager)
                                    .update_from_rest(&client_order_id, &response)
                                {
                                    let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                                        symbol: symbol.clone(),
                                        order,
                                    }));
                                }
                                return;
                            }
                            Ok(None) => {
                                let _ = tx.send(PublishEvent::AccountReconciliationStarted {
                                    symbol: symbol.clone(),
                                });
                                let instrument = TradingInstrument {
                                    symbol: symbol.clone(),
                                    tick_size: order.tick_size,
                                };
                                match user_data_stream::reconcile_account_state(
                                    client.clone(),
                                    std::slice::from_ref(&instrument),
                                    order_manager.clone(),
                                    tx.clone(),
                                )
                                .await
                                {
                                    Ok(()) => {
                                        warn!(
                                            %symbol,
                                            %client_order_id,
                                            "cancel returned unknown order; account state reconciled"
                                        );
                                        return;
                                    }
                                    Err(recovery_error) => {
                                        error!(
                                            %symbol,
                                            %client_order_id,
                                            ?recovery_error,
                                            "cancel recovery failed; execution latched off"
                                        );
                                        let _ = tx.send(PublishEvent::ExecutionUncertain {
                                            symbol: symbol.clone(),
                                        });
                                    }
                                }
                            }
                            Err(query_error) => {
                                error!(
                                    %symbol,
                                    %client_order_id,
                                    ?query_error,
                                    "order cancellation outcome is unresolved; execution latched off"
                                );
                                let _ = tx.send(PublishEvent::ExecutionUncertain {
                                    symbol: symbol.clone(),
                                });
                            }
                        }
                    } else if let Some(order) =
                        lock_recover(&order_manager).update_cancel_fail(&client_order_id, &error)
                    {
                        let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                            symbol: symbol.clone(),
                            order,
                        }));
                    }
                    let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Error(LiveError::with(
                        ErrorKind::OrderError,
                        error.into(),
                    ))));
                }
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
        sync::{mpsc::unbounded_channel, oneshot},
        time,
    };

    use super::{BinanceConfig, BinanceFutures};
    use crate::ports::{ExecutionVenue, PublishEvent};

    #[test]
    fn unknown_exchange_submit_errors_are_ambiguous_by_default() {
        let error = super::BinanceFuturesError::OrderError {
            code: -1099,
            msg: "execution status unknown".to_string(),
        };
        assert!(error.submission_is_ambiguous());
    }

    #[test]
    fn known_exchange_rejections_are_definitive() {
        for code in [-5022, -2027, -2019, -1015, -1008] {
            let error = super::BinanceFuturesError::OrderError {
                code,
                msg: "request rejected".to_string(),
            };
            assert!(!error.submission_is_ambiguous(), "code {code}");
        }
    }

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

    async fn write_order_response(stream: &mut TcpStream, client_order_id: &str, status: &str) {
        let response_body = serde_json::json!({
            "clientOrderId": client_order_id,
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
            "status": status,
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
    }

    async fn write_json_response(stream: &mut TcpStream, status: &str, body: &str) {
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn position_cap_rejection_is_definitive_and_published_before_terminal_order() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let body = serde_json::json!({
                "code": -2027,
                "msg": "Exceeded the maximum allowable position at current leverage."
            })
            .to_string();
            write_json_response(&mut stream, "400 Bad Request", &body).await;
            request
        });

        let connector = BinanceFutures::new(BinanceConfig {
            public_stream_url: "ws://127.0.0.1/".to_string(),
            private_stream_url: "ws://127.0.0.1/{listen_key}".to_string(),
            api_url: format!("http://{address}"),
            order_prefix: "strategy-a".to_string(),
            api_key: "key".to_string(),
            secret: "secret".to_string(),
            allow_test_endpoints: true,
        })
        .unwrap();
        let mut order = Order::new(
            41,
            1000,
            0.1,
            1.0,
            Side::Buy,
            OrdType::Limit,
            TimeInForce::GTC,
        );
        order.req = Status::New;
        let (tx, mut rx) = unbounded_channel();
        connector.submit("btcusdt".to_string(), order, 0.001, tx);

        let first = time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            first,
            PublishEvent::SubmissionCapacityRejected {
                ref symbol,
                side: Side::Buy,
                code: -2027,
            } if symbol == "btcusdt"
        ));

        let mut terminal_seen = false;
        let mut uncertain_seen = false;
        time::timeout(Duration::from_secs(1), async {
            while !terminal_seen {
                match rx.recv().await {
                    Some(PublishEvent::LiveEvent(LiveEvent::Order { order, .. })) => {
                        terminal_seen = order.status == Status::Expired;
                    }
                    Some(PublishEvent::ExecutionUncertain { .. }) => uncertain_seen = true,
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await
        .unwrap();

        assert!(terminal_seen);
        assert!(!uncertain_seen);
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /fapi/v1/order "));
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
            allow_test_endpoints: true,
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
        connector.submit("btcusdt".to_string(), order, 0.001, tx);

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

    #[tokio::test]
    async fn submit_is_tracked_before_the_async_http_request_starts() {
        let connector = BinanceFutures::new(BinanceConfig {
            public_stream_url: "ws://127.0.0.1/".to_string(),
            private_stream_url: "ws://127.0.0.1/{listen_key}".to_string(),
            api_url: "http://127.0.0.1:9".to_string(),
            order_prefix: "strategy-a".to_string(),
            api_key: "key".to_string(),
            secret: "secret".to_string(),
            allow_test_endpoints: true,
        })
        .unwrap();
        let mut order = Order::new(
            84,
            1_000,
            0.1,
            0.001,
            Side::Buy,
            OrdType::Limit,
            TimeInForce::GTC,
        );
        order.req = Status::New;
        let (tx, _rx) = unbounded_channel();

        connector.submit("btcusdt".to_string(), order, 0.001, tx);

        assert_eq!(connector.open_orders("btcusdt").len(), 1);
    }

    #[tokio::test]
    async fn cancel_waits_until_the_in_flight_submission_is_resolved() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let (post_seen_tx, post_seen_rx) = oneshot::channel();
        let (release_post_tx, mut release_post_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut post_stream, _) = listener.accept().await.unwrap();
            let post = read_http_request(&mut post_stream).await;
            let client_order_id = form_value(request_body(&post), "newClientOrderId")
                .unwrap()
                .to_string();
            server_requests.lock().await.push(post);
            post_seen_tx.send(()).unwrap();

            tokio::select! {
                release = &mut release_post_rx => {
                    release.unwrap();
                    write_order_response(&mut post_stream, &client_order_id, "NEW").await;
                    let (mut cancel_stream, _) = listener.accept().await.unwrap();
                    let cancel = read_http_request(&mut cancel_stream).await;
                    server_requests.lock().await.push(cancel);
                    write_order_response(&mut cancel_stream, &client_order_id, "CANCELED").await;
                }
                accepted = listener.accept() => {
                    let (mut cancel_stream, _) = accepted.unwrap();
                    let cancel = read_http_request(&mut cancel_stream).await;
                    server_requests.lock().await.push(cancel);
                    write_order_response(&mut cancel_stream, &client_order_id, "CANCELED").await;
                    release_post_rx.await.unwrap();
                    write_order_response(&mut post_stream, &client_order_id, "NEW").await;
                }
            }
        });

        let connector = BinanceFutures::new(BinanceConfig {
            public_stream_url: "ws://127.0.0.1/".to_string(),
            private_stream_url: "ws://127.0.0.1/{listen_key}".to_string(),
            api_url: format!("http://{address}"),
            order_prefix: "strategy-a".to_string(),
            api_key: "key".to_string(),
            secret: "secret".to_string(),
            allow_test_endpoints: true,
        })
        .unwrap();
        let mut order = Order::new(
            85,
            1_000,
            0.1,
            1.0,
            Side::Buy,
            OrdType::Limit,
            TimeInForce::GTC,
        );
        order.req = Status::New;
        let (tx, _rx) = unbounded_channel();

        connector.submit("btcusdt".to_string(), order.clone(), 0.001, tx.clone());
        post_seen_rx.await.unwrap();
        connector.cancel("btcusdt".to_string(), order, tx);

        time::sleep(Duration::from_millis(75)).await;
        assert_eq!(
            requests.lock().await.len(),
            1,
            "DELETE must not race ahead of the unresolved POST"
        );
        release_post_tx.send(()).unwrap();
        time::timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
        let requests = requests.lock().await;
        assert!(requests[0].starts_with("POST /fapi/v1/order "));
        assert!(requests[1].starts_with("DELETE /fapi/v1/order "));
    }

    #[tokio::test]
    async fn unknown_cancel_result_reconciles_account_state_without_latching_execution() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for step in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                requests.push(request);
                match step {
                    0 => {
                        let body = serde_json::json!({
                            "code": -2011,
                            "msg": "Unknown order sent."
                        })
                        .to_string();
                        write_json_response(&mut stream, "400 Bad Request", &body).await;
                    }
                    1 => {
                        let body = serde_json::json!({
                            "code": -2013,
                            "msg": "Order does not exist."
                        })
                        .to_string();
                        write_json_response(&mut stream, "400 Bad Request", &body).await;
                    }
                    2 | 3 => write_json_response(&mut stream, "200 OK", "[]").await,
                    _ => unreachable!(),
                }
            }
            requests
        });

        let connector = BinanceFutures::new(BinanceConfig {
            public_stream_url: "ws://127.0.0.1/".to_string(),
            private_stream_url: "ws://127.0.0.1/{listen_key}".to_string(),
            api_url: format!("http://{address}"),
            order_prefix: "strategy-a".to_string(),
            api_key: "key".to_string(),
            secret: "secret".to_string(),
            allow_test_endpoints: true,
        })
        .unwrap();
        let mut order = Order::new(
            86,
            1_000,
            0.1,
            1.0,
            Side::Buy,
            OrdType::Limit,
            TimeInForce::GTC,
        );
        order.status = Status::New;
        super::lock_recover(&connector.order_manager)
            .prepare_client_order_id("btcusdt".to_string(), order.clone())
            .unwrap();
        let (tx, mut rx) = unbounded_channel();

        connector.cancel("btcusdt".to_string(), order, tx);

        let mut reconciliation_started = false;
        let mut ready = false;
        let mut uncertain = false;
        time::timeout(Duration::from_secs(2), async {
            while !ready {
                match rx.recv().await {
                    Some(PublishEvent::AccountReconciliationStarted { symbol }) => {
                        assert_eq!(symbol, "btcusdt");
                        reconciliation_started = true;
                    }
                    Some(PublishEvent::AccountSnapshotReady { symbol }) => {
                        assert_eq!(symbol, "btcusdt");
                        ready = true;
                    }
                    Some(PublishEvent::ExecutionUncertain { .. }) => uncertain = true,
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await
        .unwrap();

        assert!(reconciliation_started);
        assert!(ready);
        assert!(
            !uncertain,
            "confirmed reconciliation must not latch execution"
        );
        assert!(connector.open_orders("btcusdt").is_empty());
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests[0].starts_with("DELETE /fapi/v1/order "));
        assert!(requests[1].starts_with("GET /fapi/v1/order?"));
        assert!(requests[2].starts_with("GET /fapi/v1/openOrders?"));
        assert!(requests[3].starts_with("GET /fapi/v3/positionRisk?"));
    }
}
