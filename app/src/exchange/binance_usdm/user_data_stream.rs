use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use hftbacktest::prelude::*;
use tokio::{select, sync::mpsc::UnboundedSender, time};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, warn};

use crate::{
    exchange::binance_usdm::{
        BinanceFuturesError, lock_recover,
        orders::SharedOrderManager,
        protocol::stream::{EventStream, Stream},
        rest::BinanceFuturesClient,
        transport::connect_websocket,
    },
    ports::{PublishEvent, TradingInstrument},
};

pub struct UserDataStream {
    instruments: Vec<TradingInstrument>,
    client: BinanceFuturesClient,
    ev_tx: UnboundedSender<PublishEvent>,
    order_manager: SharedOrderManager,
}

impl UserDataStream {
    pub fn new(
        client: BinanceFuturesClient,
        ev_tx: UnboundedSender<PublishEvent>,
        order_manager: SharedOrderManager,
        instruments: Vec<TradingInstrument>,
    ) -> Self {
        Self {
            instruments,
            client,
            ev_tx,
            order_manager,
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
                    if position.position_side != "BOTH" {
                        return Err(BinanceFuturesError::UnsupportedPositionMode);
                    }
                    if !position.position_amount.is_finite() {
                        return Err(BinanceFuturesError::InvalidAccountState);
                    }
                    self.ev_tx
                        .send(PublishEvent::LiveEvent(LiveEvent::Position {
                            symbol: position.symbol,
                            qty: position.position_amount,
                            exch_ts: data.transaction_time.saturating_mul(1_000_000),
                        }))
                        .map_err(|_| BinanceFuturesError::PublishSinkClosed)?;
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
                    Err(error) => return Err(error),
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

        let mut last_ping = Instant::now();
        reconcile_account_state(
            self.client.clone(),
            &self.instruments,
            self.order_manager.clone(),
            self.ev_tx.clone(),
        )
        .await?;

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

pub async fn reconcile_account_state(
    client: BinanceFuturesClient,
    instruments: &[TradingInstrument],
    order_manager: SharedOrderManager,
    ev_tx: UnboundedSender<PublishEvent>,
) -> Result<(), BinanceFuturesError> {
    let mut open_order_snapshots = Vec::with_capacity(instruments.len());
    for instrument in instruments {
        open_order_snapshots.push(client.get_open_orders(&instrument.symbol).await?);
    }

    let configured_symbols: hashbrown::HashSet<_> = instruments
        .iter()
        .map(|instrument| instrument.symbol.as_str())
        .collect();
    let mut positions = HashMap::new();
    for position in client.get_position_information().await? {
        if configured_symbols.contains(position.symbol.as_str()) {
            if position.position_side != "BOTH" {
                return Err(BinanceFuturesError::UnsupportedPositionMode);
            }
            if !position.position_amount.is_finite()
                || positions
                    .insert(position.symbol.clone(), position)
                    .is_some()
            {
                return Err(BinanceFuturesError::InvalidAccountState);
            }
        }
    }

    for (instrument, open_orders) in instruments.iter().zip(open_order_snapshots.iter()) {
        let recovered = lock_recover(&order_manager).reconcile_open_orders(
            &instrument.symbol,
            instrument.tick_size,
            open_orders,
        )?;
        let (qty, exch_ts) = positions
            .remove(&instrument.symbol)
            .map(|position| {
                (
                    position.position_amount,
                    position.update_time.saturating_mul(1_000_000),
                )
            })
            .unwrap_or((0.0, 0));
        ev_tx
            .send(PublishEvent::LiveEvent(LiveEvent::Position {
                symbol: instrument.symbol.clone(),
                qty,
                exch_ts,
            }))
            .map_err(|_| BinanceFuturesError::PublishSinkClosed)?;
        for order in recovered {
            ev_tx
                .send(PublishEvent::LiveEvent(LiveEvent::Order {
                    symbol: instrument.symbol.clone(),
                    order,
                }))
                .map_err(|_| BinanceFuturesError::PublishSinkClosed)?;
        }
        ev_tx
            .send(PublishEvent::AccountSnapshotReady {
                symbol: instrument.symbol.clone(),
            })
            .map_err(|_| BinanceFuturesError::PublishSinkClosed)?;
    }
    Ok(())
}
