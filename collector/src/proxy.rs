use anyhow::{Context, Result, bail};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const MAX_CONNECT_RESPONSE_SIZE: usize = 8 * 1024;

pub async fn connect(proxy: &str, target: &str) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(proxy)
        .await
        .with_context(|| format!("failed to connect to proxy {proxy}"))?;
    stream
        .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .await
        .context("failed to send HTTP CONNECT request")?;

    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() >= MAX_CONNECT_RESPONSE_SIZE {
            bail!("proxy CONNECT response headers are too large");
        }
        response.push(
            stream
                .read_u8()
                .await
                .context("proxy closed the CONNECT tunnel")?,
        );
    }
    validate_connect_response(&response)?;
    Ok(stream)
}

fn validate_connect_response(response: &[u8]) -> Result<()> {
    let response = std::str::from_utf8(response).context("proxy response is not UTF-8")?;
    let status = response.lines().next().context("proxy response is empty")?;
    let code = status
        .split_whitespace()
        .nth(1)
        .context("proxy response has no HTTP status code")?;
    if code != "200" {
        bail!("proxy CONNECT failed: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_successful_connect_response() {
        validate_connect_response(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
    }

    #[test]
    fn rejects_failed_connect_response() {
        let error = validate_connect_response(b"HTTP/1.1 403 Forbidden\r\n\r\n").unwrap_err();
        assert!(format!("{error:#}").contains("403 Forbidden"));
    }
}
