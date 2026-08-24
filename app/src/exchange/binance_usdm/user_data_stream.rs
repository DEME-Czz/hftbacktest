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
use tracing::{debug, error, warn};

use crate::{
    exchange::binance_usdm::{
        BinanceFuturesError, SharedSymbolSet, lock_recover,
        orders::SharedOrderManager,
        protocol::stream::{EventStream, Stream},
        rest::BinanceFuturesClient,
        transport::connect_websocket,
    },
    ports::PublishEvent,
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
            EventStream::DepthUpdate(_) | EventStream::Trade(_) => {
                warn!("ignoring public event received on private user stream");
            }
            EventStream::ListenKeyExpired(_) => return Err(BinanceFuturesError::ListenKeyExpired),
            EventStream::AccountUpdate(data) => {
                for position in data.account.position {
                    let _ = self
                        .ev_tx
                        .send(PublishEvent::LiveEvent(LiveEvent::Position {
                            symbol: position.symbol,
                            qty: position.position_amount,
                            exch_ts: data.transaction_time * 1_000_000,
                        }));
                }
            }
            EventStream::OrderTradeUpdate(data) => {
                match lock_recover(&self.order_manager).update_from_ws(&data) {
                    Ok(Some(order)) => {
                        let _ = self.ev_tx.send(PublishEvent::LiveEvent(LiveEvent::Order {
                            symbol: data.order.symbol,
                            order,
                        }));
                    }
                    Ok(None) => {}
                    Err(BinanceFuturesError::PrefixUnmatched) => {}
                    Err(error) => error!(?error, ?data, "couldn't update order from user stream"),
                }
            }
            EventStream::TradeLite(_data) => {}
        }
        Ok(())
    }

    pub async fn connect(&mut self, url: &str) -> Result<(), BinanceFuturesError> {
        let (ws_stream, _) = connect_websocket(url).await?;
        let (mut write, mut read) = ws_stream.split();
        let mut interval = time::interval(Duration::from_secs(60 * 30));
        let mut ping_checker = time::interval(Duration::from_secs(10));

        let symbols: HashSet<_> = lock_recover(&self.symbols).iter().cloned().collect();
        let client = self.client.clone();
        let ev_tx = self.ev_tx.clone();
        let mut last_ping = Instant::now();

        tokio::spawn(async move {
            if let Err(error) =
                get_position_information(client.clone(), symbols, ev_tx.clone()).await
            {
                error!(?error, "couldn't get initial position information");
            }
        });

        loop {
            select! {
                _ = interval.tick() => {
                    lock_recover(&self.order_manager).gc();
                    let client_ = self.client.clone();
                    tokio::spawn(async move {
                        if let Err(error) = client_.keepalive_user_data_stream().await {
                            error!(?error, "failed to keep user data stream alive");
                        }
                    });
                }
                _ = ping_checker.tick() => {
                    if last_ping.elapsed() > Duration::from_secs(300) {
                        warn!("user data stream ping timeout");
                        return Err(BinanceFuturesError::ConnectionInterrupted);
                    }
                }
                msg = self.symbol_rx.recv() => {
                    match msg {
                        Ok(symbol) => {
                            let client = self.client.clone();
                            let ev_tx = self.ev_tx.clone();
                            tokio::spawn(async move {
                                // Always reconcile the newly registered symbol. This removes the
                                // startup race where the private stream snapshots the symbol set
                                // before main() calls register(), which otherwise leaves live
                                // execution waiting forever for its initial position state.
                                let mut symbols = HashSet::new();
                                symbols.insert(symbol.clone());
                                if let Err(error) = get_position_information(client, symbols, ev_tx).await {
                                    error!(?error, %symbol, "couldn't reconcile registered symbol position");
                                }
                            });
                        }
                        Err(RecvError::Closed) => return Ok(()),
                        Err(RecvError::Lagged(num)) => error!(num, "user stream symbol registrations were missed"),
                    }
                }
                message = read.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<Stream>(&text) {
                            Ok(Stream::EventStream(stream)) => self.process_message(stream)?,
                            Ok(Stream::Result(result)) => debug!(?result, "user stream response received"),
                            Err(error) => error!(?error, %text, "couldn't parse user stream"),
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        write.send(Message::Pong(data)).await?;
                        last_ping = Instant::now();
                    }
                    Some(Ok(Message::Close(close_frame))) => {
                        return Err(BinanceFuturesError::ConnectionAbort(
                            close_frame.map(|f| f.to_string()).unwrap_or_default(),
                        ));
                    }
                    Some(Ok(Message::Binary(_)))
                    | Some(Ok(Message::Frame(_)))
                    | Some(Ok(Message::Pong(_))) => {}
                    Some(Err(error)) => return Err(error.into()),
                    None => return Err(BinanceFuturesError::ConnectionInterrupted),
                }
            }
        }
    }
}

pub async fn get_position_information(
    client: BinanceFuturesClient,
    mut symbols: HashSet<String>,
    ev_tx: UnboundedSender<PublishEvent>,
) -> Result<(), BinanceFuturesError> {
    let position_information = client.get_position_information().await?;
    position_information.into_iter().for_each(|position| {
        if symbols.remove(&position.symbol) {
            let _ = ev_tx.send(PublishEvent::LiveEvent(LiveEvent::Position {
                symbol: position.symbol,
                qty: position.position_amount,
                exch_ts: position.update_time * 1_000_000,
            }));
        }
    });
    for symbol in symbols {
        let _ = ev_tx.send(PublishEvent::LiveEvent(LiveEvent::Position {
            symbol,
            qty: 0.0,
            exch_ts: 0,
        }));
    }
    Ok(())
}
