use std::{
    io,
    io::ErrorKind,
    time::{Duration, Instant},
};

use anyhow::Error;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::TcpStream,
    select,
    sync::mpsc::{UnboundedSender, unbounded_channel},
    time::interval,
};
use tokio_tungstenite::{
    client_async_tls_with_config,
    tungstenite::{Bytes, Message, Utf8Bytes, client::IntoClientRequest},
};
use tracing::{error, warn};

use crate::proxy;

pub async fn fetch_symbol_list() -> Result<Vec<String>, reqwest::Error> {
    Ok(reqwest::Client::new()
        .get("https://fapi.binance.com/fapi/v1/exchangeInfo")
        .header("Accept", "application/json")
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?
        .get("symbols")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter(|j_symbol| j_symbol.get("contractType").unwrap().as_str().unwrap() == "PERPETUAL")
        .map(|j_symbol| {
            j_symbol
                .get("symbol")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect())
}

pub async fn fetch_depth_snapshot(symbol: &str, proxy: Option<&str>) -> Result<String> {
    let mut client = reqwest::Client::builder();
    if let Some(proxy) = proxy {
        client = client.proxy(reqwest::Proxy::all(format!("http://{proxy}"))?);
    }
    let client = client.build().context("failed to build HTTP client")?;
    let url = format!("https://fapi.binance.com/fapi/v1/depth?symbol={symbol}&limit=1000");
    let mut last_error = None;
    for attempt in 1..=5 {
        match fetch_depth_snapshot_once(&client, &url).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) => {
                warn!(%symbol, attempt, ?error, "depth snapshot request failed; retrying");
                last_error = Some(error);
                if attempt < 5 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
    Err(last_error.expect("at least one snapshot request was attempted"))
}

async fn fetch_depth_snapshot_once(client: &reqwest::Client, url: &str) -> Result<String> {
    client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await
        .map_err(Into::into)
}

pub async fn connect(
    url: &str,
    proxy_addr: Option<&str>,
    ws_tx: UnboundedSender<(DateTime<Utc>, Utf8Bytes)>,
) -> Result<(), anyhow::Error> {
    let request = url.into_client_request()?;
    let host = request
        .uri()
        .host()
        .ok_or_else(|| anyhow::anyhow!("WebSocket URL has no host"))?;
    let port = request.uri().port_u16().unwrap_or(443);
    let target = format!("{host}:{port}");
    let socket = match proxy_addr {
        Some(proxy_addr) => proxy::connect(proxy_addr, &target).await?,
        None => TcpStream::connect(&target).await?,
    };
    let (ws_stream, _) = client_async_tls_with_config(request, socket, None, None).await?;
    let (mut write, mut read) = ws_stream.split();
    let (tx, mut rx) = unbounded_channel::<Bytes>();

    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if write.send(Message::Pong(data)).await.is_err() {
                let _ = write.close().await;
                return;
            }
        }
    });

    let mut last_ping = Instant::now();
    let mut checker = interval(Duration::from_secs(10));

    loop {
        select! {
            msg = read.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let recv_time = Utc::now();
                    if ws_tx.send((recv_time, text)).is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Binary(_))) => {}
                Some(Ok(Message::Ping(data))) => {
                    if tx.send(data).is_err() {
                        return Err(Error::from(io::Error::new(
                            ErrorKind::ConnectionAborted,
                            "closed",
                        )));
                    }
                    last_ping = Instant::now();
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(close_frame))) => {
                    warn!(?close_frame, "closed");
                    return Err(Error::from(io::Error::new(
                        ErrorKind::ConnectionAborted,
                        "closed",
                    )));
                }
                Some(Ok(Message::Frame(_))) => {}
                Some(Err(e)) => {
                    return Err(Error::from(e));
                }
                None => {
                    break;
                }
            },
            _ = checker.tick() => {
                if last_ping.elapsed() > Duration::from_secs(300) {
                    warn!("Ping timeout.");
                    return Err(Error::from(io::Error::new(
                        ErrorKind::TimedOut,
                        "Ping",
                    )));
                }
            }
        }
    }
    Ok(())
}

pub async fn keep_connection(
    streams: Vec<String>,
    symbol_list: Vec<String>,
    proxy: Option<String>,
    ws_tx: UnboundedSender<(DateTime<Utc>, Utf8Bytes)>,
) {
    let mut error_count = 0;
    loop {
        let connect_time = Instant::now();
        let streams_str = symbol_list
            .iter()
            .flat_map(|pair| {
                streams
                    .iter()
                    .cloned()
                    .map(|stream| {
                        stream
                            .replace("$symbol", pair.to_lowercase().as_str())
                            .to_string()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .join("/");
        if let Err(error) = connect(
            &format!("wss://fstream.binance.com/stream?streams={streams_str}"),
            proxy.as_deref(),
            ws_tx.clone(),
        )
        .await
        {
            error!(?error, "websocket error");
            error_count += 1;
            if connect_time.elapsed() > Duration::from_secs(30) {
                error_count = 0;
            }
            if error_count > 3 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            } else if error_count > 10 {
                tokio::time::sleep(Duration::from_secs(5)).await;
            } else if error_count > 20 {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        } else {
            break;
        }
    }
}
