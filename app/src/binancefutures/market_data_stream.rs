use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use hftbacktest::prelude::*;
use tokio::{
    select,
    sync::{
        broadcast::{Receiver, error::RecvError},
        mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    },
    time,
};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, warn};

use crate::{
    binancefutures::{
        BinanceFuturesError,
        msg::{
            rest,
            stream,
            stream::{EventStream, Stream},
        },
        rest::BinanceFuturesClient,
    },
    connector::PublishEvent,
    utils::{connect_websocket, generate_rand_string, parse_depth, parse_px_qty_tup},
};

const BROADCAST_TARGET: u64 = 0;

pub struct MarketDataStream {
    client: BinanceFuturesClient,
    ev_tx: UnboundedSender<PublishEvent>,
    symbol_rx: Receiver<String>,
    pending_depth_messages: HashMap<String, Vec<stream::Depth>>,
    prev_u: HashMap<String, i64>,
    rest_tx: UnboundedSender<(String, rest::Depth)>,
    rest_rx: UnboundedReceiver<(String, rest::Depth)>,
}

impl MarketDataStream {
    pub fn new(
        client: BinanceFuturesClient,
        ev_tx: UnboundedSender<PublishEvent>,
        symbol_rx: Receiver<String>,
    ) -> Self {
        let (rest_tx, rest_rx) = unbounded_channel();
        Self {
            client,
            ev_tx,
            symbol_rx,
            pending_depth_messages: Default::default(),
            prev_u: Default::default(),
            rest_tx,
            rest_rx,
        }
    }

    fn request_snapshot(&self, symbol: String) {
        let client = self.client.clone();
        let rest_tx = self.rest_tx.clone();
        tokio::spawn(async move {
            match client.get_depth(&symbol).await {
                Ok(depth) => {
                    let _ = rest_tx.send((symbol, depth));
                }
                Err(error) => {
                    error!(?error, %symbol, "failed to fetch Binance depth snapshot");
                }
            }
        });
    }

    fn emit_depth_levels(
        &self,
        symbol: &str,
        transaction_time: i64,
        bids: Vec<(String, String)>,
        asks: Vec<(String, String)>,
    ) {
        let Ok((bids, asks)) = parse_depth(bids, asks) else {
            error!(%symbol, "failed to parse Binance depth levels");
            return;
        };

        self.ev_tx
            .send(PublishEvent::BatchStart(BROADCAST_TARGET))
            .unwrap();

        for (px, qty) in bids {
            self.ev_tx
                .send(PublishEvent::LiveEvent(LiveEvent::Feed {
                    symbol: symbol.to_string(),
                    event: Event {
                        ev: LOCAL_BID_DEPTH_EVENT,
                        exch_ts: transaction_time * 1_000_000,
                        local_ts: Utc::now().timestamp_nanos_opt().unwrap(),
                        order_id: 0,
                        px,
                        qty,
                        ival: 0,
                        fval: 0.0,
                    },
                }))
                .unwrap();
        }

        for (px, qty) in asks {
            self.ev_tx
                .send(PublishEvent::LiveEvent(LiveEvent::Feed {
                    symbol: symbol.to_string(),
                    event: Event {
                        ev: LOCAL_ASK_DEPTH_EVENT,
                        exch_ts: transaction_time * 1_000_000,
                        local_ts: Utc::now().timestamp_nanos_opt().unwrap(),
                        order_id: 0,
                        px,
                        qty,
                        ival: 0,
                        fval: 0.0,
                    },
                }))
                .unwrap();
        }

        self.ev_tx
            .send(PublishEvent::BatchEnd(BROADCAST_TARGET))
            .unwrap();
    }

    fn emit_depth_update(&self, data: &stream::Depth) {
        self.emit_depth_levels(
            &data.symbol,
            data.transaction_time,
            data.bids.clone(),
            data.asks.clone(),
        );
    }

    fn handle_depth_update(&mut self, data: stream::Depth) {
        let symbol = data.symbol.clone();

        if let Some(previous_u) = self.prev_u.get(&symbol).copied() {
            // Binance requires pu == previous u after the initial snapshot handoff.
            if data.prev_update_id != previous_u {
                warn!(
                    %symbol,
                    previous_u,
                    pu = data.prev_update_id,
                    "Binance depth sequence gap detected; resynchronizing"
                );
                self.prev_u.remove(&symbol);
                self.pending_depth_messages.insert(symbol.clone(), vec![data]);
                self.request_snapshot(symbol);
                return;
            }
            self.emit_depth_update(&data);
            self.prev_u.insert(symbol, data.last_update_id);
            return;
        }

        let pending = self
            .pending_depth_messages
            .entry(symbol.clone())
            .or_default();
        let request_snapshot = pending.is_empty();
        pending.push(data);
        if request_snapshot {
            self.request_snapshot(symbol);
        }
    }

    fn process_snapshot(&mut self, symbol: String, data: rest::Depth) {
        // Clear the local book before applying a fresh REST snapshot.
        self.ev_tx
            .send(PublishEvent::LiveEvent(LiveEvent::Feed {
                symbol: symbol.clone(),
                event: Event {
                    ev: LOCAL_DEPTH_CLEAR_EVENT,
                    exch_ts: data.transaction_time * 1_000_000,
                    local_ts: Utc::now().timestamp_nanos_opt().unwrap(),
                    order_id: 0,
                    px: 0.0,
                    qty: 0.0,
                    ival: 0,
                    fval: 0.0,
                },
            }))
            .unwrap();

        self.emit_depth_levels(
            &symbol,
            data.transaction_time,
            data.bids,
            data.asks,
        );

        let mut pending = self
            .pending_depth_messages
            .remove(&symbol)
            .unwrap_or_default();
        pending.retain(|event| event.last_update_id >= data.last_update_id);

        let Some(first_index) = pending.iter().position(|event| {
            event.first_update_id <= data.last_update_id
                && event.last_update_id >= data.last_update_id
        }) else {
            // No buffered event bridges the snapshot yet; wait for a new update and fetch again.
            self.prev_u.remove(&symbol);
            return;
        };

        let mut previous_u = None;
        for event in pending.into_iter().skip(first_index) {
            if let Some(prev) = previous_u
                && event.prev_update_id != prev
            {
                warn!(%symbol, prev, pu = event.prev_update_id, "gap in buffered depth updates");
                self.prev_u.remove(&symbol);
                self.pending_depth_messages
                    .entry(symbol.clone())
                    .or_default()
                    .push(event);
                self.request_snapshot(symbol.clone());
                return;
            }
            self.emit_depth_update(&event);
            previous_u = Some(event.last_update_id);
        }

        if let Some(last_u) = previous_u {
            self.prev_u.insert(symbol, last_u);
        }
    }

    fn process_message(&mut self, stream: EventStream) {
        match stream {
            EventStream::DepthUpdate(data) => self.handle_depth_update(data),
            EventStream::Trade(data) => {
                if data.type_ != "MARKET" {
                    return;
                }
                match parse_px_qty_tup(data.price, data.qty) {
                    Ok((px, qty)) => {
                        self.ev_tx
                            .send(PublishEvent::LiveEvent(LiveEvent::Feed {
                                symbol: data.symbol,
                                event: Event {
                                    ev: if data.is_the_buyer_the_market_maker {
                                        LOCAL_SELL_TRADE_EVENT
                                    } else {
                                        LOCAL_BUY_TRADE_EVENT
                                    },
                                    exch_ts: data.transaction_time * 1_000_000,
                                    local_ts: Utc::now().timestamp_nanos_opt().unwrap(),
                                    order_id: 0,
                                    px,
                                    qty,
                                    ival: 0,
                                    fval: 0.0,
                                },
                            }))
                            .unwrap();
                    }
                    Err(error) => error!(?error, "failed to parse Binance trade stream"),
                }
            }
            _ => unreachable!(),
        }
    }

    pub async fn connect(&mut self, url: &str) -> Result<(), BinanceFuturesError> {
        let (ws_stream, _) = connect_websocket(url).await?;
        let (mut write, mut read) = ws_stream.split();
        let mut ping_checker = time::interval(Duration::from_secs(10));
        let mut last_ping = Instant::now();

        loop {
            select! {
                Some((symbol, data)) = self.rest_rx.recv() => {
                    self.process_snapshot(symbol, data);
                }
                _ = ping_checker.tick() => {
                    if last_ping.elapsed() > Duration::from_secs(300) {
                        return Err(BinanceFuturesError::ConnectionInterrupted);
                    }
                }
                msg = self.symbol_rx.recv() => match msg {
                    Ok(symbol) => {
                        let id = generate_rand_string(16);
                        write.send(Message::Text(format!(r#"{{
                            "method":"SUBSCRIBE",
                            "params":["{symbol}@trade","{symbol}@depth@100ms"],
                            "id":"{id}"
                        }}"#).into())).await?;
                    }
                    Err(RecvError::Closed) => return Ok(()),
                    Err(RecvError::Lagged(num)) => {
                        error!(num, "Binance subscription requests were missed");
                    }
                },
                message = read.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<Stream>(&text) {
                            Ok(Stream::EventStream(stream)) => self.process_message(stream),
                            Ok(Stream::Result(result)) => debug!(?result, "Binance subscription response"),
                            Err(error) => error!(?error, %text, "failed to parse Binance stream"),
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        write.send(Message::Pong(data)).await?;
                        last_ping = Instant::now();
                    }
                    Some(Ok(Message::Close(close_frame))) => {
                        return Err(BinanceFuturesError::ConnectionAbort(
                            close_frame.map(|f| f.to_string()).unwrap_or_default()
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
