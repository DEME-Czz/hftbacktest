use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

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
    exchange::binance_usdm::{
        BinanceFuturesError, SharedSymbolSet,
        id::generate_random_id,
        lock_recover, now_ns,
        protocol::{
            parse_depth, parse_px_qty, rest, stream,
            stream::{EventStream, Stream},
        },
        rest::BinanceFuturesClient,
        transport::connect_websocket,
    },
    ports::PublishEvent,
};

const MAX_PENDING_DEPTH_MESSAGES: usize = 4_096;
type SnapshotResult = Result<rest::Depth, BinanceFuturesError>;

pub struct MarketDataStream {
    client: BinanceFuturesClient,
    ev_tx: UnboundedSender<PublishEvent>,
    symbols: SharedSymbolSet,
    symbol_rx: Receiver<String>,
    pending_depth_messages: HashMap<String, Vec<stream::Depth>>,
    prev_u: HashMap<String, i64>,
    rest_tx: UnboundedSender<(String, SnapshotResult)>,
    rest_rx: UnboundedReceiver<(String, SnapshotResult)>,
}

impl MarketDataStream {
    pub fn new(
        client: BinanceFuturesClient,
        ev_tx: UnboundedSender<PublishEvent>,
        symbols: SharedSymbolSet,
        symbol_rx: Receiver<String>,
    ) -> Self {
        let (rest_tx, rest_rx) = unbounded_channel();
        Self {
            client,
            ev_tx,
            symbols,
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
            let mut last_error = None;
            for delay_ms in [0_u64, 100, 250, 500, 1_000] {
                if delay_ms > 0 {
                    time::sleep(Duration::from_millis(delay_ms)).await;
                }
                match client.get_depth(&symbol).await {
                    Ok(depth) => {
                        let _ = rest_tx.send((symbol, Ok(depth)));
                        return;
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            let error = last_error.unwrap_or(BinanceFuturesError::ConnectionInterrupted);
            error!(?error, %symbol, "Binance depth snapshot retries exhausted");
            let _ = rest_tx.send((symbol, Err(error)));
        });
    }

    fn publish(&self, event: PublishEvent) -> Result<(), BinanceFuturesError> {
        self.ev_tx
            .send(event)
            .map_err(|_| BinanceFuturesError::PublishSinkClosed)
    }

    fn reset_connection_state(&mut self) {
        self.pending_depth_messages.clear();
        self.prev_u.clear();
        let (rest_tx, rest_rx) = unbounded_channel();
        self.rest_tx = rest_tx;
        self.rest_rx = rest_rx;
    }

    fn emit_depth_levels(
        &self,
        symbol: &str,
        transaction_time: i64,
        bids: Vec<(String, String)>,
        asks: Vec<(String, String)>,
    ) -> Result<(), BinanceFuturesError> {
        let Ok((bids, asks)) = parse_depth(bids, asks) else {
            error!(%symbol, "failed to parse Binance depth levels");
            return Ok(());
        };

        self.publish(PublishEvent::BatchStart)?;

        for (px, qty) in bids {
            self.publish(PublishEvent::LiveEvent(LiveEvent::Feed {
                symbol: symbol.to_string(),
                event: Event {
                    ev: LOCAL_BID_DEPTH_EVENT,
                    exch_ts: transaction_time * 1_000_000,
                    local_ts: now_ns(),
                    order_id: 0,
                    px,
                    qty,
                    ival: 0,
                    fval: 0.0,
                },
            }))?;
        }

        for (px, qty) in asks {
            self.publish(PublishEvent::LiveEvent(LiveEvent::Feed {
                symbol: symbol.to_string(),
                event: Event {
                    ev: LOCAL_ASK_DEPTH_EVENT,
                    exch_ts: transaction_time * 1_000_000,
                    local_ts: now_ns(),
                    order_id: 0,
                    px,
                    qty,
                    ival: 0,
                    fval: 0.0,
                },
            }))?;
        }

        self.publish(PublishEvent::BatchEnd {
            received_at: Instant::now(),
        })
    }

    fn emit_depth_update(&self, data: &stream::Depth) -> Result<(), BinanceFuturesError> {
        self.emit_depth_levels(
            &data.symbol,
            data.transaction_time,
            data.bids.clone(),
            data.asks.clone(),
        )
    }

    fn handle_depth_update(&mut self, data: stream::Depth) -> Result<(), BinanceFuturesError> {
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
                self.pending_depth_messages
                    .insert(symbol.clone(), vec![data]);
                self.request_snapshot(symbol);
                return Ok(());
            }
            self.emit_depth_update(&data)?;
            self.prev_u.insert(symbol, data.last_update_id);
            return Ok(());
        }

        let pending = self
            .pending_depth_messages
            .entry(symbol.clone())
            .or_default();
        let request_snapshot = pending.is_empty();
        if pending.len() >= MAX_PENDING_DEPTH_MESSAGES {
            return Err(BinanceFuturesError::DepthBufferOverflow);
        }
        pending.push(data);
        if request_snapshot {
            self.request_snapshot(symbol);
        }
        Ok(())
    }

    fn process_snapshot(
        &mut self,
        symbol: String,
        data: rest::Depth,
    ) -> Result<(), BinanceFuturesError> {
        let mut pending = self
            .pending_depth_messages
            .remove(&symbol)
            .unwrap_or_default();
        pending.retain(|event| event.last_update_id >= data.last_update_id);

        let Some(first_index) = pending.iter().position(|event| {
            event.first_update_id <= data.last_update_id
                && event.last_update_id >= data.last_update_id
        }) else {
            // A snapshot is not tradable until a buffered diff bridges its update id. Preserve
            // the buffer and retry only when at least one diff exists; otherwise the next diff
            // will trigger a new snapshot request.
            self.prev_u.remove(&symbol);
            let should_retry = !pending.is_empty();
            self.pending_depth_messages.insert(symbol.clone(), pending);
            if should_retry {
                self.request_snapshot(symbol);
            }
            return Ok(());
        };

        if let Some(gap) = pending[first_index..]
            .windows(2)
            .find(|pair| pair[1].prev_update_id != pair[0].last_update_id)
        {
            warn!(
                %symbol,
                prev = gap[0].last_update_id,
                pu = gap[1].prev_update_id,
                "gap in buffered depth updates"
            );
            self.prev_u.remove(&symbol);
            self.pending_depth_messages.insert(symbol.clone(), pending);
            self.request_snapshot(symbol);
            return Ok(());
        }

        // Publish atomically only after continuity has been established. This prevents strategy
        // decisions from observing a REST snapshot that cannot be connected to the live stream.
        self.publish(PublishEvent::LiveEvent(LiveEvent::Feed {
            symbol: symbol.clone(),
            event: Event {
                ev: LOCAL_DEPTH_CLEAR_EVENT,
                exch_ts: data.transaction_time * 1_000_000,
                local_ts: now_ns(),
                order_id: 0,
                px: 0.0,
                qty: 0.0,
                ival: 0,
                fval: 0.0,
            },
        }))?;
        self.emit_depth_levels(&symbol, data.transaction_time, data.bids, data.asks)?;

        let mut previous_u = None;
        for event in pending.into_iter().skip(first_index) {
            self.emit_depth_update(&event)?;
            previous_u = Some(event.last_update_id);
        }

        if let Some(last_u) = previous_u {
            self.prev_u.insert(symbol, last_u);
        }
        Ok(())
    }

    fn process_message(&mut self, stream: EventStream) -> Result<(), BinanceFuturesError> {
        match stream {
            EventStream::DepthUpdate(data) => self.handle_depth_update(data)?,
            EventStream::Trade(data) => {
                if data.type_ != "MARKET" {
                    return Ok(());
                }
                match parse_px_qty(data.price, data.qty) {
                    Ok((px, qty)) => {
                        self.publish(PublishEvent::LiveEvent(LiveEvent::Feed {
                            symbol: data.symbol,
                            event: Event {
                                ev: if data.is_the_buyer_the_market_maker {
                                    LOCAL_SELL_TRADE_EVENT
                                } else {
                                    LOCAL_BUY_TRADE_EVENT
                                },
                                exch_ts: data.transaction_time * 1_000_000,
                                local_ts: now_ns(),
                                order_id: 0,
                                px,
                                qty,
                                ival: 0,
                                fval: 0.0,
                            },
                        }))?;
                    }
                    Err(error) => error!(?error, "failed to parse Binance trade stream"),
                }
            }
            other => warn!(
                ?other,
                "ignoring private event received on public market stream"
            ),
        }
        Ok(())
    }

    pub async fn connect(&mut self, url: &str) -> Result<(), BinanceFuturesError> {
        let (ws_stream, _) = connect_websocket(url).await?;
        let (mut write, mut read) = ws_stream.split();
        self.reset_connection_state();
        let mut ping_checker = time::interval(Duration::from_secs(10));
        let mut last_ping = Instant::now();
        let mut subscribed = HashSet::new();

        let registered_symbols: Vec<_> = lock_recover(&self.symbols).iter().cloned().collect();
        for symbol in registered_symbols {
            if subscribed.insert(symbol.clone()) {
                let id = generate_random_id(16);
                write
                    .send(Message::Text(subscription_request(&symbol, &id).into()))
                    .await?;
            }
        }

        loop {
            select! {
                Some((symbol, result)) = self.rest_rx.recv() => {
                    self.process_snapshot(symbol, result?)?;
                }
                _ = ping_checker.tick() => {
                    if last_ping.elapsed() > Duration::from_secs(300) {
                        return Err(BinanceFuturesError::ConnectionInterrupted);
                    }
                }
                msg = self.symbol_rx.recv() => match msg {
                    Ok(symbol) => {
                        if subscribed.insert(symbol.clone()) {
                            let id = generate_random_id(16);
                            write.send(Message::Text(subscription_request(&symbol, &id).into())).await?;
                        }
                    }
                    Err(RecvError::Closed) => return Ok(()),
                    Err(RecvError::Lagged(num)) => {
                        error!(num, "Binance subscription requests were missed");
                    }
                },
                message = read.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<Stream>(&text) {
                            Ok(Stream::EventStream(stream)) => self.process_message(stream)?,
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

fn subscription_request(symbol: &str, id: &str) -> String {
    format!(
        r#"{{
        "method":"SUBSCRIBE",
        "params":["{symbol}@trade","{symbol}@depth@100ms"],
        "id":"{id}"
    }}"#
    )
}

#[cfg(test)]
mod tests {
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        time::Duration,
    };

    use futures_util::StreamExt;
    use tokio::{
        net::TcpListener,
        sync::{broadcast, mpsc::unbounded_channel},
        time::timeout,
    };
    use tokio_tungstenite::accept_async;

    use super::{MAX_PENDING_DEPTH_MESSAGES, MarketDataStream};
    use crate::exchange::binance_usdm::{SharedSymbolSet, rest::BinanceFuturesClient};

    #[test]
    fn closed_event_sink_does_not_panic() {
        let client = BinanceFuturesClient::new("http://127.0.0.1:9", "", "").unwrap();
        let (event_tx, event_rx) = unbounded_channel();
        drop(event_rx);
        let (symbol_tx, _) = broadcast::channel(4);
        let symbols: SharedSymbolSet = Default::default();
        let stream = MarketDataStream::new(client, event_tx, symbols, symbol_tx.subscribe());

        let result = catch_unwind(AssertUnwindSafe(|| {
            stream.emit_depth_levels(
                "btcusdt",
                1,
                vec![("100.0".to_string(), "1.0".to_string())],
                vec![("101.0".to_string(), "1.0".to_string())],
            )
        }));

        assert!(
            result.is_ok(),
            "a closed event sink must not panic the stream task"
        );
        assert!(matches!(
            result.unwrap(),
            Err(crate::exchange::binance_usdm::BinanceFuturesError::PublishSinkClosed)
        ));
    }

    #[tokio::test]
    async fn snapshot_is_not_published_until_a_buffered_update_bridges_it() {
        use crate::exchange::binance_usdm::protocol::{rest, stream};

        let client = BinanceFuturesClient::new("http://127.0.0.1:9", "", "").unwrap();
        let (event_tx, mut event_rx) = unbounded_channel();
        let (symbol_tx, _) = broadcast::channel(4);
        let symbols: SharedSymbolSet = Default::default();
        let mut market = MarketDataStream::new(client, event_tx, symbols, symbol_tx.subscribe());
        market.pending_depth_messages.insert(
            "btcusdt".to_string(),
            vec![stream::Depth {
                transaction_time: 2,
                event_time: 2,
                symbol: "btcusdt".to_string(),
                first_update_id: 200,
                last_update_id: 210,
                prev_update_id: 199,
                bids: vec![("100.0".to_string(), "1.0".to_string())],
                asks: vec![("101.0".to_string(), "1.0".to_string())],
            }],
        );

        market
            .process_snapshot(
                "btcusdt".to_string(),
                rest::Depth {
                    last_update_id: 100,
                    event_time: 1,
                    transaction_time: 1,
                    bids: vec![("99.0".to_string(), "1.0".to_string())],
                    asks: vec![("102.0".to_string(), "1.0".to_string())],
                },
            )
            .unwrap();

        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn reconnect_replays_every_registered_symbol() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut subscriptions = Vec::new();
            for _ in 0..2 {
                let (socket, _) = listener.accept().await.unwrap();
                let mut websocket = accept_async(socket).await.unwrap();
                let mut params = Vec::new();
                while params.len() < 4 {
                    let message = timeout(Duration::from_millis(500), websocket.next())
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                    let text = message.into_text().unwrap();
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    params.extend(
                        value["params"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|value| value.as_str().unwrap().to_string()),
                    );
                }
                params.sort();
                subscriptions.push(params);
                websocket.close(None).await.unwrap();
            }
            subscriptions
        });

        let client = BinanceFuturesClient::new("http://127.0.0.1:9", "", "").unwrap();
        let (event_tx, _event_rx) = unbounded_channel();
        let (symbol_tx, _) = broadcast::channel(4);
        let symbols: SharedSymbolSet = Default::default();
        {
            let mut symbols = symbols.lock().unwrap();
            symbols.insert("btcusdt".to_string());
            symbols.insert("ethusdt".to_string());
        }
        let mut stream = MarketDataStream::new(client, event_tx, symbols, symbol_tx.subscribe());
        symbol_tx.send("btcusdt".to_string()).unwrap();

        let url = format!("ws://{address}");
        assert!(stream.connect(&url).await.is_err());
        assert!(stream.connect(&url).await.is_err());

        let expected = vec![
            "btcusdt@depth@100ms".to_string(),
            "btcusdt@trade".to_string(),
            "ethusdt@depth@100ms".to_string(),
            "ethusdt@trade".to_string(),
        ];
        assert_eq!(server.await.unwrap(), vec![expected.clone(), expected]);
    }

    #[tokio::test]
    async fn pending_depth_buffer_is_bounded_when_snapshot_never_arrives() {
        use crate::exchange::binance_usdm::protocol::stream;

        let client = BinanceFuturesClient::new("http://127.0.0.1:9", "", "").unwrap();
        let (event_tx, _event_rx) = unbounded_channel();
        let (symbol_tx, _) = broadcast::channel(4);
        let symbols: SharedSymbolSet = Default::default();
        let mut market = MarketDataStream::new(client, event_tx, symbols, symbol_tx.subscribe());

        for update_id in 1..=MAX_PENDING_DEPTH_MESSAGES as i64 {
            market
                .handle_depth_update(stream::Depth {
                    transaction_time: update_id,
                    event_time: update_id,
                    symbol: "btcusdt".to_string(),
                    first_update_id: update_id,
                    last_update_id: update_id,
                    prev_update_id: update_id - 1,
                    bids: Vec::new(),
                    asks: Vec::new(),
                })
                .unwrap();
        }
        let overflow = market.handle_depth_update(stream::Depth {
            transaction_time: 1,
            event_time: 1,
            symbol: "btcusdt".to_string(),
            first_update_id: 1,
            last_update_id: 1,
            prev_update_id: 0,
            bids: Vec::new(),
            asks: Vec::new(),
        });

        assert!(matches!(
            overflow,
            Err(crate::exchange::binance_usdm::BinanceFuturesError::DepthBufferOverflow)
        ));
    }
}
