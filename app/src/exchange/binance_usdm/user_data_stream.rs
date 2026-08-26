use std::{
    collections::HashMap,
    future::Future,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use hftbacktest::prelude::*;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    task::JoinHandle,
    time,
};
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

enum UserStreamFrame {
    Text(String, Instant),
    Activity(Instant),
    Terminal(BinanceFuturesError),
}

struct UserStreamReaderGuard(JoinHandle<()>);

impl Drop for UserStreamReaderGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn bootstrap_recovery_barrier<T, R, E, Recovery, Process, Ready>(
    frames: &mut UnboundedReceiver<T>,
    recovery: Recovery,
    mut process: Process,
    ready: Ready,
) -> Result<(), E>
where
    Recovery: Future<Output = Result<R, E>>,
    Process: FnMut(T) -> Result<(), E>,
    Ready: FnOnce(R) -> Result<(), E>,
{
    tokio::pin!(recovery);
    let mut buffered = Vec::new();
    let mut frames_open = true;
    let recovered = loop {
        if !frames_open {
            break recovery.await?;
        }
        select! {
            result = &mut recovery => break result?,
            frame = frames.recv() => match frame {
                Some(frame) => buffered.push(frame),
                None => frames_open = false,
            }
        }
    };

    for frame in buffered {
        process(frame)?;
    }
    while let Ok(frame) = frames.try_recv() {
        process(frame)?;
    }
    ready(recovered)
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

    fn process_frame(
        &self,
        frame: UserStreamFrame,
        last_activity: &mut Instant,
    ) -> Result<(), BinanceFuturesError> {
        match frame {
            UserStreamFrame::Text(text, received_at) => {
                *last_activity = received_at;
                match serde_json::from_str::<Stream>(&text) {
                    Ok(Stream::EventStream(stream)) => self.process_message(stream)?,
                    Ok(Stream::Result(result)) => {
                        debug!(?result, "user stream response received");
                    }
                    Err(error) => error!(?error, %text, "couldn't parse user stream"),
                }
            }
            UserStreamFrame::Activity(received_at) => *last_activity = received_at,
            UserStreamFrame::Terminal(error) => return Err(error),
        }
        Ok(())
    }

    pub async fn connect(&mut self, url: &str) -> Result<(), BinanceFuturesError> {
        let (ws_stream, _) = connect_websocket(url).await?;
        let (mut write, mut read) = ws_stream.split();
        let (frame_tx, mut frame_rx) = unbounded_channel();
        let reader_task = tokio::spawn(async move {
            loop {
                let (frame, terminal) = match read.next().await {
                    Some(Ok(Message::Text(text))) => (
                        UserStreamFrame::Text(text.to_string(), Instant::now()),
                        false,
                    ),
                    Some(Ok(Message::Ping(data))) => match write.send(Message::Pong(data)).await {
                        Ok(()) => (UserStreamFrame::Activity(Instant::now()), false),
                        Err(error) => (UserStreamFrame::Terminal(error.into()), true),
                    },
                    Some(Ok(Message::Pong(_)))
                    | Some(Ok(Message::Binary(_)))
                    | Some(Ok(Message::Frame(_))) => {
                        (UserStreamFrame::Activity(Instant::now()), false)
                    }
                    Some(Ok(Message::Close(close_frame))) => (
                        UserStreamFrame::Terminal(BinanceFuturesError::ConnectionAbort(
                            close_frame
                                .map(|frame| frame.to_string())
                                .unwrap_or_default(),
                        )),
                        true,
                    ),
                    Some(Err(error)) => (UserStreamFrame::Terminal(error.into()), true),
                    None => (
                        UserStreamFrame::Terminal(BinanceFuturesError::ConnectionInterrupted),
                        true,
                    ),
                };
                if frame_tx.send(frame).is_err() || terminal {
                    break;
                }
            }
        });
        let _reader_guard = UserStreamReaderGuard(reader_task);
        let mut interval = time::interval(Duration::from_secs(60 * 30));
        let mut activity_checker = time::interval(Duration::from_secs(10));
        let mut last_activity = Instant::now();

        bootstrap_recovery_barrier(
            &mut frame_rx,
            reconcile_account_state_without_ready(
                self.client.clone(),
                &self.instruments,
                self.order_manager.clone(),
                self.ev_tx.clone(),
            ),
            |frame| self.process_frame(frame, &mut last_activity),
            |()| publish_account_snapshot_ready(&self.instruments, &self.ev_tx),
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
                _ = activity_checker.tick() => {
                    if last_activity.elapsed() > Duration::from_secs(300) {
                        warn!("user data stream activity timeout");
                        return Err(BinanceFuturesError::ConnectionInterrupted);
                    }
                }
                frame = frame_rx.recv() => match frame {
                    Some(frame) => self.process_frame(frame, &mut last_activity)?,
                    None => return Err(BinanceFuturesError::ConnectionInterrupted),
                }
            }
        }
    }
}

async fn reconcile_account_state_without_ready(
    client: BinanceFuturesClient,
    instruments: &[TradingInstrument],
    order_manager: SharedOrderManager,
    ev_tx: UnboundedSender<PublishEvent>,
) -> Result<(), BinanceFuturesError> {
    reconcile_account_state_inner(client, instruments, order_manager, ev_tx, false).await
}

#[allow(dead_code)]
pub async fn reconcile_account_state(
    client: BinanceFuturesClient,
    instruments: &[TradingInstrument],
    order_manager: SharedOrderManager,
    ev_tx: UnboundedSender<PublishEvent>,
) -> Result<(), BinanceFuturesError> {
    reconcile_account_state_inner(client, instruments, order_manager, ev_tx, true).await
}

async fn reconcile_account_state_inner(
    client: BinanceFuturesClient,
    instruments: &[TradingInstrument],
    order_manager: SharedOrderManager,
    ev_tx: UnboundedSender<PublishEvent>,
    publish_ready: bool,
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

    let snapshots: Vec<_> = instruments
        .iter()
        .zip(open_order_snapshots)
        .map(|(instrument, open_orders)| {
            (instrument.symbol.clone(), instrument.tick_size, open_orders)
        })
        .collect();
    let reconciled = lock_recover(&order_manager).reconcile_all_open_orders(&snapshots)?;

    for (instrument, (_, recovered)) in instruments.iter().zip(reconciled) {
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
        if publish_ready {
            publish_account_snapshot_ready(std::slice::from_ref(instrument), &ev_tx)?;
        }
    }
    Ok(())
}

fn publish_account_snapshot_ready(
    instruments: &[TradingInstrument],
    ev_tx: &UnboundedSender<PublishEvent>,
) -> Result<(), BinanceFuturesError> {
    for instrument in instruments {
        ev_tx
            .send(PublishEvent::AccountSnapshotReady {
                symbol: instrument.symbol.clone(),
            })
            .map_err(|_| BinanceFuturesError::PublishSinkClosed)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tokio::sync::{mpsc::unbounded_channel, oneshot};

    use super::bootstrap_recovery_barrier;

    #[tokio::test]
    async fn bootstrap_recovery_replays_private_frames_before_ready() {
        let (frame_tx, mut frame_rx) = unbounded_channel();
        let (release_recovery_tx, release_recovery_rx) = oneshot::channel();
        let observed = RefCell::new(Vec::new());

        let producer = async move {
            frame_tx.send("account").unwrap();
            tokio::task::yield_now().await;
            frame_tx.send("order").unwrap();
            release_recovery_tx.send(()).unwrap();
        };
        let recovery = async {
            release_recovery_rx.await.unwrap();
            observed.borrow_mut().push("snapshot");
            Ok::<_, ()>(())
        };
        let barrier = bootstrap_recovery_barrier(
            &mut frame_rx,
            recovery,
            |frame| {
                observed.borrow_mut().push(frame);
                Ok(())
            },
            |()| {
                observed.borrow_mut().push("ready");
                Ok(())
            },
        );

        let ((), result) = tokio::join!(producer, barrier);
        result.unwrap();

        assert_eq!(
            observed.into_inner(),
            vec!["snapshot", "account", "order", "ready"]
        );
    }
}
