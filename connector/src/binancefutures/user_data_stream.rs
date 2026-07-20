use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use hftbacktest::prelude::*;
use tokio::{
    select,
    sync::{
        broadcast::{Receiver, error::RecvError},
        mpsc::UnboundedSender,
    },
    time,
};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::{
    binancefutures::{
        BinanceFuturesError, SharedSymbolSet,
        events::{balance_events, fill_event},
        msg::stream::{EventStream, Stream},
        ordermanager::SharedOrderManager,
        rest::BinanceFuturesClient,
    },
    connector::PublishEvent,
    utils::connect_websocket,
};

pub struct UserDataStream {
    symbols: SharedSymbolSet,
    client: BinanceFuturesClient,
    ev_tx: UnboundedSender<PublishEvent>,
    order_manager: SharedOrderManager,
    symbol_rx: Receiver<String>,
}

impl UserDataStream {
    pub fn new(
        client: BinanceFuturesClient,
        ev_tx: UnboundedSender<PublishEvent>,
        order_manager: SharedOrderManager,
        symbols: SharedSymbolSet,
        symbol_rx: Receiver<String>,
    ) -> Self {
        Self {
            symbols,
            client,
            ev_tx,
            order_manager,
            symbol_rx,
        }
    }

    pub async fn get_listen_key(&self) -> Result<String, BinanceFuturesError> {
        Ok(self.client.start_user_data_stream().await?)
    }

    fn process_message(&self, stream: EventStream) -> Result<(), BinanceFuturesError> {
        match stream {
            EventStream::DepthUpdate(_) | EventStream::Trade(_) => unreachable!(),
            EventStream::ListenKeyExpired(_) => {
                return Err(BinanceFuturesError::ListenKeyExpired);
            }
            EventStream::AccountUpdate(data) => {
                let symbols = self.symbols.lock().unwrap();
                for event in balance_events(
                    &symbols,
                    data.account
                        .balance
                        .iter()
                        .map(|balance| (balance.asset.as_str(), balance.wallet_balance)),
                    data.transaction_time * 1_000_000,
                ) {
                    self.ev_tx.send(PublishEvent::LiveEvent(event)).unwrap();
                }
                drop(symbols);

                for position in data.account.position {
                    self.ev_tx
                        .send(PublishEvent::LiveEvent(LiveEvent::Position {
                            symbol: position.symbol,
                            qty: position.position_amount,
                            exch_ts: data.transaction_time * 1_000_000,
                        }))
                        .unwrap();
                }
            }
            EventStream::OrderTradeUpdate(data) => {
                match self.order_manager.lock().unwrap().update_from_ws(&data) {
                    Ok(Some(order)) => {
                        let fill = fill_event(&data);
                        self.ev_tx
                            .send(PublishEvent::LiveEvent(LiveEvent::Order {
                                symbol: data.order.symbol,
                                order,
                            }))
                            .unwrap();
                        if let Some(fill) = fill {
                            self.ev_tx.send(PublishEvent::LiveEvent(fill)).unwrap();
                        }
                    }
                    Ok(None) => {
                        // This order is already deleted.
                    }
                    Err(BinanceFuturesError::PrefixUnmatched) => {
                        // This order is not created by this connector.
                    }
                    Err(BinanceFuturesError::OrderNotFound) => {
                        // User data streams are account-wide. In particular, the startup
                        // cancel-all request can produce updates for orders created by a previous
                        // connector process that used the same configured prefix. The exact
                        // client-order-id map, rather than the reusable prefix, is the ownership
                        // boundary for this process.
                        debug!(
                            symbol = %data.order.symbol,
                            client_order_id = %data.order.client_order_id,
                            exchange_order_id = data.order.order_id,
                            status = ?data.order.order_status,
                            "Ignoring an order update that is not tracked by this connector process."
                        );
                    }
                    Err(error) => {
                        error!(
                            ?error,
                            ?data,
                            "Couldn't update the order from OrderTradeUpdate message."
                        );
                    }
                }
            }
            EventStream::TradeLite(_data) => {
                // Since this message does not include the order status, additional logic is
                // required to fully utilize it. To reduce latency— which first needs to be
                // measured—a new logic must be implemented to reconstruct the order status and open
                // position by using the last filled quantity and reconciling it with data from the
                // ORDER_TRADE_UPDATE message.
            }
        }
        Ok(())
    }

    pub async fn connect(&mut self, url: &str) -> Result<(), BinanceFuturesError> {
        let (ws_stream, _) = connect_websocket(url).await?;
        let (mut write, mut read) = ws_stream.split();
        let mut interval = time::interval(Duration::from_secs(60 * 30));
        let mut ping_checker = time::interval(Duration::from_secs(10));

        let symbols: HashSet<_> = self.symbols.lock().unwrap().iter().cloned().collect();
        let client = self.client.clone();
        let order_manager = self.order_manager.clone();
        let ev_tx = self.ev_tx.clone();
        let mut last_ping = Instant::now();

        tokio::spawn(async move {
            // Cancel all orders before connecting to the stream in order to start with the
            // clean state.
            for symbol in &symbols {
                if let Err(error) = cancel_all(
                    client.clone(),
                    symbol.clone(),
                    order_manager.clone(),
                    ev_tx.clone(),
                )
                .await
                {
                    error!(?error, %symbol, "Couldn't cancel all orders.");
                }
            }

            // Fetches the initial states such as positions and open orders.
            if let Err(error) = get_initial_state(client.clone(), symbols, ev_tx.clone()).await {
                error!(?error, "Couldn't get initial account state.");
            }
        });

        loop {
            select! {
                _ = interval.tick() => {
                    self.order_manager
                        .lock()
                        .unwrap()
                        .gc();
                    let client_ = self.client.clone();
                    tokio::spawn(async move {
                        if let Err(error) = client_.keepalive_user_data_stream().await {
                            error!(?error, "Failed keepalive user data stream.");
                            // todo: reset the connection.
                        }
                    });
                }
                _ = ping_checker.tick() => {
                    if last_ping.elapsed() > Duration::from_secs(300) {
                        warn!("Ping timeout.");
                        return Err(BinanceFuturesError::ConnectionInterrupted);
                    }
                }
                msg = self.symbol_rx.recv() => {
                    match msg {
                        Ok(symbol) => {
                            let client = self.client.clone();
                            let order_manager = self.order_manager.clone();
                            let ev_tx = self.ev_tx.clone();

                            tokio::spawn(async move {
                                if let Err(error) = cancel_all(
                                    client.clone(),
                                    symbol.clone(),
                                    order_manager.clone(),
                                    ev_tx.clone()
                                ).await {
                                    error!(?error, %symbol, "Couldn't cancel all orders.");
                                }

                                // The user stream is normally connected before any bot registers
                                // an instrument. Its connection-time snapshot therefore cannot map
                                // an asset balance to this newly registered symbol. Refresh the
                                // account state here so late registrations receive both position
                                // and quote-asset wallet balance without waiting for a future
                                // ACCOUNT_UPDATE event.
                                if let Err(error) = get_initial_state(
                                    client,
                                    HashSet::from([symbol.clone()]),
                                    ev_tx,
                                ).await {
                                    error!(?error, %symbol, "Couldn't refresh account state after instrument registration.");
                                }
                            });
                        }
                        Err(RecvError::Closed) => {
                            return Ok(());
                        }
                        Err(RecvError::Lagged(num)) => {
                            error!("{num} subscription requests were missed.");
                        }
                    }
                }
                message = read.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<Stream>(&text) {
                            Ok(Stream::EventStream(stream)) => {
                                self.process_message(stream)?;
                            }
                            Ok(Stream::Result(result)) => {
                                debug!(?result, "Subscription request response is received.");
                            }
                            Err(error) => {
                                error!(?error, %text, "Couldn't parse Stream.");
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        write.send(Message::Pong(data)).await?;
                        last_ping = Instant::now();
                    }
                    Some(Ok(Message::Close(close_frame))) => {
                        return Err(BinanceFuturesError::ConnectionAbort(
                            close_frame.map(|f| f.to_string()).unwrap_or(String::new())
                        ));
                    }
                    Some(Ok(Message::Binary(_)))
                    | Some(Ok(Message::Frame(_)))
                    | Some(Ok(Message::Pong(_))) => {}
                    Some(Err(error)) => {
                        return Err(BinanceFuturesError::from(error));
                    }
                    None => {
                        return Err(BinanceFuturesError::ConnectionInterrupted);
                    }
                }
            }
        }
    }
}

pub async fn cancel_all(
    client: BinanceFuturesClient,
    symbol: String,
    order_manager: SharedOrderManager,
    ev_tx: UnboundedSender<PublishEvent>,
) -> Result<(), BinanceFuturesError> {
    // todo: rate-limit throttling.
    client.cancel_all_orders(&symbol).await?;
    let orders = order_manager.lock().unwrap().cancel_all_from_rest(&symbol);
    for order in orders {
        ev_tx
            .send(PublishEvent::LiveEvent(LiveEvent::Order {
                symbol: symbol.clone(),
                order,
            }))
            .unwrap();
    }
    Ok(())
}

pub async fn get_initial_state(
    client: BinanceFuturesClient,
    mut symbols: HashSet<String>,
    ev_tx: UnboundedSender<PublishEvent>,
) -> Result<(), BinanceFuturesError> {
    // todo: rate-limit throttling.
    info!(?symbols, "Requesting Binance Futures account snapshot.");
    let account = client.get_account_information().await?;
    let balance_ts = account
        .assets
        .iter()
        .map(|asset| asset.update_time)
        .max()
        .unwrap_or_default()
        * 1_000_000;
    let balance_events = balance_events(
        &symbols,
        account
            .assets
            .iter()
            .map(|asset| (asset.asset.as_str(), asset.wallet_balance)),
        balance_ts,
    );
    if balance_events.is_empty() {
        warn!(
            ?symbols,
            assets = ?account.assets.iter().map(|asset| asset.asset.as_str()).collect::<Vec<_>>(),
            "Binance account snapshot contained no balance matching the registered symbols."
        );
    }
    for event in balance_events {
        ev_tx.send(PublishEvent::LiveEvent(event)).unwrap();
    }

    info!(
        registered_symbols = symbols.len(),
        assets = account.assets.len(),
        positions = account.positions.len(),
        "Received Binance Futures account snapshot."
    );

    account.positions.into_iter().for_each(|position| {
        symbols.remove(&position.symbol);
        ev_tx
            .send(PublishEvent::LiveEvent(LiveEvent::Position {
                symbol: position.symbol,
                qty: position.position_amount,
                exch_ts: position.update_time * 1_000_000,
            }))
            .unwrap();
    });
    for symbol in symbols {
        ev_tx
            .send(PublishEvent::LiveEvent(LiveEvent::Position {
                symbol,
                qty: 0.0,
                exch_ts: 0,
            }))
            .unwrap();
    }
    Ok(())
}
