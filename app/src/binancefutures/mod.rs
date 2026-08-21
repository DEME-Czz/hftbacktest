mod market_data_stream;
mod msg;
mod ordermanager;
mod rest;
mod user_data_stream;

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

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
    binancefutures::{
        ordermanager::{OrderManager, SharedOrderManager},
        rest::BinanceFuturesClient,
    },
    connector::{Connector, ConnectorBuilder, GetOrders, PublishEvent},
    utils::{ExponentialBackoff, Retry},
};

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
    #[error("Config: {0:?}")]
    Config(#[from] toml::de::Error),
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

#[derive(Deserialize)]
pub struct Config {
    pub public_stream_url: String,
    pub private_stream_url: String,
    pub api_url: String,
    #[serde(default)]
    pub order_prefix: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub secret: String,
}

type SharedSymbolSet = Arc<Mutex<HashSet<String>>>;

pub struct BinanceFutures {
    config: Config,
    symbols: SharedSymbolSet,
    order_manager: SharedOrderManager,
    client: BinanceFuturesClient,
    symbol_tx: Sender<String>,
}

impl BinanceFutures {
    pub fn connect_market_data_stream(&mut self, ev_tx: UnboundedSender<PublishEvent>) {
        let base_url = self.config.public_stream_url.clone();
        let client = self.client.clone();
        let symbol_tx = self.symbol_tx.clone();
        // Subscribe before spawning. This guarantees register() has at least one receiver
        // immediately after this method returns and removes the startup race.
        let initial_symbol_rx = self.symbol_tx.subscribe();

        tokio::spawn(async move {
            let mut initial_symbol_rx = Some(initial_symbol_rx);
            let _ = Retry::new(ExponentialBackoff::default())
                .error_handler(|error: BinanceFuturesError| {
                    error!(?error, "market data stream connection interrupted");
                    let _ = ev_tx.send(PublishEvent::LiveEvent(LiveEvent::Error(LiveError::with(
                        ErrorKind::ConnectionInterrupted,
                        error.into(),
                    ))));
                    Ok(())
                })
                .retry(|| async {
                    let symbol_rx = initial_symbol_rx
                        .take()
                        .unwrap_or_else(|| symbol_tx.subscribe());
                    let mut stream = market_data_stream::MarketDataStream::new(
                        client.clone(),
                        ev_tx.clone(),
                        symbol_rx,
                    );
                    debug!(%base_url, "connecting Binance public market stream");
                    stream.connect(&base_url).await?;
                    Ok(())
                })
                .await;
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

    /// Starts only public market data. Collector must use this path so API credentials,
    /// when present in the config, never cause a private user-data connection.
    pub fn run_market_data_only(&mut self, ev_tx: UnboundedSender<PublishEvent>) {
        self.connect_market_data_stream(ev_tx);
    }
}

impl ConnectorBuilder for BinanceFutures {
    type Error = BinanceFuturesError;

    fn build_from(config: &str) -> Result<Self, Self::Error> {
        let config: Config = toml::from_str(config)?;
        if !config.private_stream_url.contains("{listen_key}") {
            return Err(BinanceFuturesError::InvalidRequest);
        }
        let order_manager = Arc::new(Mutex::new(OrderManager::new(&config.order_prefix)));
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
}

impl Connector for BinanceFutures {
    fn register(&mut self, symbol: String) {
        let symbol = symbol.to_lowercase();
        let mut symbols = self.symbols.lock().unwrap();
        if symbols.insert(symbol.clone()) {
            // A missing receiver is not a fatal condition. The symbol remains in the authoritative
            // symbol set and can be replayed/re-registered by the runtime after reconnect.
            if self.symbol_tx.send(symbol.clone()).is_err() {
                warn!(%symbol, "symbol registered before a stream receiver was ready");
            }
        }
    }

    fn order_manager(&self) -> Arc<Mutex<dyn GetOrders + Send + 'static>> {
        self.order_manager.clone()
    }

    fn run(&mut self, ev_tx: UnboundedSender<PublishEvent>) {
        self.connect_market_data_stream(ev_tx.clone());
        if !self.config.api_key.is_empty() && !self.config.secret.is_empty() {
            self.connect_user_data_stream(ev_tx);
        }
    }

    fn submit(&self, symbol: String, mut order: Order, tx: UnboundedSender<PublishEvent>) {
        let client = self.client.clone();
        let order_manager = self.order_manager.clone();
        tokio::spawn(async move {
            let client_order_id = order_manager
                .lock()
                .unwrap()
                .prepare_client_order_id(symbol.clone(), order.clone());

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
                            if let Some(order) = order_manager
                                .lock()
                                .unwrap()
                                .update_from_rest(&client_order_id, &resp)
                            {
                                let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                                    symbol,
                                    order,
                                }));
                            }
                        }
                        Err(error) => {
                            if let Some(order) = order_manager
                                .lock()
                                .unwrap()
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
            let client_order_id = order_manager
                .lock()
                .unwrap()
                .get_client_order_id(&symbol, order.order_id);

            if let Some(client_order_id) = client_order_id {
                match client.cancel_order(&client_order_id, &symbol).await {
                    Ok(resp) => {
                        if let Some(order) = order_manager
                            .lock()
                            .unwrap()
                            .update_from_rest(&client_order_id, &resp)
                        {
                            let _ = tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                                symbol,
                                order,
                            }));
                        }
                    }
                    Err(error) => {
                        if let Some(order) = order_manager
                            .lock()
                            .unwrap()
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
