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
        orders::{OrderManager, SharedOrderManager},
        rest::BinanceFuturesClient,
    },
    ports::{ExecutionVenue, MarketDataSource, PublishEvent},
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
    pub fn new(config: BinanceConfig) -> Self {
        let order_manager = Arc::new(Mutex::new(OrderManager::new(&config.order_prefix)));
        let client = BinanceFuturesClient::new(&config.api_url, &config.api_key, &config.secret);
        let (symbol_tx, _) = broadcast::channel(500);

        Self {
            config,
            symbols: Default::default(),
            order_manager,
            client,
            symbol_tx,
        }
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

    pub fn connect_user_data_stream(&self, ev_tx: UnboundedSender<PublishEvent>) {
        let url_template = self.config.private_stream_url.clone();
        let client = self.client.clone();
        let order_manager = self.order_manager.clone();
        let instruments = self.symbols.clone();
        let symbol_tx = self.symbol_tx.clone();

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
                        symbol_tx.subscribe(),
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
    fn start_account_stream(&self, ev_tx: UnboundedSender<PublishEvent>) {
        self.connect_user_data_stream(ev_tx);
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
