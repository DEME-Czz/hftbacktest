use std::fmt::Write;

use chrono::Utc;
use hftbacktest::types::{OrdType, Side, TimeInForce};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

use super::{
    BinanceFuturesError,
    protocol::{
        rest::{
            self, OpenOrdersResponse, OrderResponse, OrderResponseResult, PositionInformationV3,
        },
        stream::ListenKey,
    },
};

fn side_str(side: Side) -> Result<&'static str, BinanceFuturesError> {
    match side {
        Side::Buy => Ok("BUY"),
        Side::Sell => Ok("SELL"),
        Side::None | Side::Unsupported => Err(BinanceFuturesError::InvalidRequest),
    }
}

fn order_type_str(order_type: OrdType) -> Result<&'static str, BinanceFuturesError> {
    match order_type {
        OrdType::Limit => Ok("LIMIT"),
        OrdType::Market => Ok("MARKET"),
        OrdType::Unsupported => Err(BinanceFuturesError::InvalidRequest),
    }
}

fn time_in_force_str(time_in_force: TimeInForce) -> Result<&'static str, BinanceFuturesError> {
    match time_in_force {
        TimeInForce::GTC => Ok("GTC"),
        TimeInForce::GTX => Ok("GTX"),
        TimeInForce::FOK => Ok("FOK"),
        TimeInForce::IOC => Ok("IOC"),
        TimeInForce::Unsupported => Err(BinanceFuturesError::InvalidRequest),
    }
}

fn sign_hmac_sha256(secret: &str, message: &str) -> Result<String, BinanceFuturesError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| BinanceFuturesError::InvalidRequest)?;
    mac.update(message.as_bytes());

    let bytes = mac.finalize().into_bytes();
    let mut signature = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String is infallible.
        let _ = write!(signature, "{byte:02x}");
    }
    Ok(signature)
}

fn decimal_precision(step: f64) -> Result<usize, BinanceFuturesError> {
    if !step.is_finite() || step <= 0.0 {
        return Err(BinanceFuturesError::InvalidRequest);
    }
    for precision in 0_i32..=15 {
        let scaled = step * 10_f64.powi(precision);
        let tolerance = scaled.abs().max(1.0) * f64::EPSILON * 8.0;
        if (scaled - scaled.round()).abs() <= tolerance {
            return Ok(precision as usize);
        }
    }
    Err(BinanceFuturesError::InvalidRequest)
}

#[derive(Clone)]
pub struct BinanceFuturesClient {
    client: reqwest::Client,
    url: String,
    api_key: String,
    secret: String,
}

impl BinanceFuturesClient {
    pub fn new(url: &str, api_key: &str, secret: &str) -> Result<Self, BinanceFuturesError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            url: url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            secret: secret.to_string(),
        })
    }

    async fn get_noauth<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        query: String,
    ) -> Result<T, BinanceFuturesError> {
        Ok(self
            .client
            .get(format!("{}{}?{}", self.url, path, query))
            .header("Accept", "application/json")
            .send()
            .await?
            .json()
            .await?)
    }

    fn signed_query(&self, query: &str) -> String {
        let timestamp = Utc::now().timestamp_millis();
        if query.is_empty() {
            format!("recvWindow=5000&timestamp={timestamp}")
        } else {
            format!("{query}&recvWindow=5000&timestamp={timestamp}")
        }
    }

    async fn get<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        query: String,
    ) -> Result<T, BinanceFuturesError> {
        let signed_query = self.signed_query(&query);
        let signature = sign_hmac_sha256(&self.secret, &signed_query)?;
        Ok(self
            .client
            .get(format!(
                "{}{}?{}&signature={}",
                self.url, path, signed_query, signature
            ))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .json()
            .await?)
    }

    async fn signed_body_request<T: for<'a> Deserialize<'a>>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: String,
    ) -> Result<T, BinanceFuturesError> {
        let timestamp = Utc::now().timestamp_millis();
        let mut params = body;
        if !params.is_empty() {
            params.push('&');
        }
        params.push_str(&format!("recvWindow=5000&timestamp={timestamp}"));
        let signature = sign_hmac_sha256(&self.secret, &params)?;
        params.push_str("&signature=");
        params.push_str(&signature);

        Ok(self
            .client
            .request(method, format!("{}{}", self.url, path))
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("X-MBX-APIKEY", &self.api_key)
            .body(params)
            .send()
            .await?
            .json()
            .await?)
    }

    async fn post<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        body: String,
    ) -> Result<T, BinanceFuturesError> {
        self.signed_body_request(reqwest::Method::POST, path, body)
            .await
    }

    async fn delete<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        body: String,
    ) -> Result<T, BinanceFuturesError> {
        self.signed_body_request(reqwest::Method::DELETE, path, body)
            .await
    }

    async fn user_stream_request<T: for<'a> Deserialize<'a>>(
        &self,
        method: reqwest::Method,
    ) -> Result<T, reqwest::Error> {
        self.client
            .request(method, format!("{}/fapi/v1/listenKey", self.url))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .json()
            .await
    }

    pub async fn start_user_data_stream(&self) -> Result<String, reqwest::Error> {
        let response: ListenKey = self.user_stream_request(reqwest::Method::POST).await?;
        Ok(response.listen_key)
    }

    pub async fn keepalive_user_data_stream(&self) -> Result<String, reqwest::Error> {
        let response: ListenKey = self.user_stream_request(reqwest::Method::PUT).await?;
        Ok(response.listen_key)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn submit_order(
        &self,
        client_order_id: &str,
        symbol: &str,
        side: Side,
        price: f64,
        tick_size: f64,
        quantity: f64,
        lot_size: f64,
        order_type: OrdType,
        time_in_force: TimeInForce,
    ) -> Result<OrderResponse, BinanceFuturesError> {
        let price_precision = decimal_precision(tick_size)?;
        let quantity_precision = decimal_precision(lot_size)?;
        let mut body = String::with_capacity(220);
        body.push_str("newClientOrderId=");
        body.push_str(client_order_id);
        body.push_str("&symbol=");
        body.push_str(symbol);
        body.push_str("&side=");
        body.push_str(side_str(side)?);
        if order_type == OrdType::Limit {
            body.push_str("&price=");
            body.push_str(&format!("{price:.price_precision$}"));
            body.push_str("&timeInForce=");
            body.push_str(time_in_force_str(time_in_force)?);
        }
        body.push_str("&quantity=");
        body.push_str(&format!("{quantity:.quantity_precision$}"));
        body.push_str("&type=");
        body.push_str(order_type_str(order_type)?);
        body.push_str("&newOrderRespType=RESULT");

        let response: OrderResponseResult = self.post("/fapi/v1/order", body).await?;
        match response {
            OrderResponseResult::Ok(response) => Ok(*response),
            OrderResponseResult::Err(response) => Err(BinanceFuturesError::OrderError {
                code: response.code,
                msg: response.msg,
            }),
        }
    }

    pub async fn cancel_order(
        &self,
        client_order_id: &str,
        symbol: &str,
    ) -> Result<OrderResponse, BinanceFuturesError> {
        let body = format!("symbol={symbol}&origClientOrderId={client_order_id}");
        let response: OrderResponseResult = self.delete("/fapi/v1/order", body).await?;
        match response {
            OrderResponseResult::Ok(response) => Ok(*response),
            OrderResponseResult::Err(response) => Err(BinanceFuturesError::OrderError {
                code: response.code,
                msg: response.msg,
            }),
        }
    }

    pub async fn query_order(
        &self,
        client_order_id: &str,
        symbol: &str,
    ) -> Result<Option<OrderResponse>, BinanceFuturesError> {
        let query = format!("symbol={symbol}&origClientOrderId={client_order_id}");
        let response: OrderResponseResult = self.get("/fapi/v1/order", query).await?;
        match response {
            OrderResponseResult::Ok(response) => Ok(Some(*response)),
            OrderResponseResult::Err(error) if error.code == -2013 => Ok(None),
            OrderResponseResult::Err(error) => Err(BinanceFuturesError::OrderError {
                code: error.code,
                msg: error.msg,
            }),
        }
    }

    pub async fn get_position_information(
        &self,
    ) -> Result<Vec<PositionInformationV3>, BinanceFuturesError> {
        self.get("/fapi/v3/positionRisk", String::new()).await
    }

    pub async fn get_open_orders(
        &self,
        symbol: &str,
    ) -> Result<Vec<OrderResponse>, BinanceFuturesError> {
        let response: OpenOrdersResponse = self
            .get("/fapi/v1/openOrders", format!("symbol={symbol}"))
            .await?;
        match response {
            OpenOrdersResponse::Ok(orders) => Ok(orders),
            OpenOrdersResponse::Err(error) => Err(BinanceFuturesError::OrderError {
                code: error.code,
                msg: error.msg,
            }),
        }
    }

    pub async fn get_depth(&self, symbol: &str) -> Result<rest::Depth, BinanceFuturesError> {
        self.get_noauth("/fapi/v1/depth", format!("symbol={symbol}&limit=1000"))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{side_str, sign_hmac_sha256};
    use hftbacktest::types::Side;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time,
    };

    #[test]
    fn unsupported_order_side_is_rejected_without_panicking() {
        assert!(side_str(Side::None).is_err());
        assert!(side_str(Side::Unsupported).is_err());
    }

    #[tokio::test]
    async fn unsupported_order_shape_is_rejected_without_an_http_request() {
        use hftbacktest::types::{OrdType, TimeInForce};

        let client =
            super::BinanceFuturesClient::new("http://127.0.0.1:9", "key", "secret").unwrap();
        let unsupported_type = client
            .submit_order(
                "client-id",
                "BTCUSDT",
                Side::Buy,
                100.0,
                0.1,
                0.001,
                0.001,
                OrdType::Unsupported,
                TimeInForce::GTC,
            )
            .await;
        assert!(unsupported_type.is_err());

        let unsupported_tif = client
            .submit_order(
                "client-id",
                "BTCUSDT",
                Side::Buy,
                100.0,
                0.1,
                0.001,
                0.001,
                OrdType::Limit,
                TimeInForce::Unsupported,
            )
            .await;
        assert!(unsupported_tif.is_err());
    }

    #[test]
    fn signing_matches_binance_reference_vector() {
        let signature = sign_hmac_sha256(
            "2b5eb11e18796d12d88f13dc27dbbd02c2cc51ff7059765ed9821957d82bb4d9",
            "symbol=BTCUSDT&side=BUY&type=LIMIT&quantity=1&price=9000&timeInForce=GTC&recvWindow=5000&timestamp=1591702613943",
        )
        .unwrap();

        assert_eq!(
            signature,
            "3c661234138461fcc7a7d8746c6558c9842d4e10870d2ecbedf7777cad694af9"
        );
    }

    #[tokio::test]
    async fn signed_requests_never_follow_http_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let _ = first.read(&mut request).await.unwrap();
            let redirect = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{address}/capture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            first.write_all(redirect.as_bytes()).await.unwrap();
            drop(first);

            match time::timeout(Duration::from_millis(500), listener.accept()).await {
                Ok(Ok((mut redirected, _))) => {
                    let _ = redirected.read(&mut request).await.unwrap();
                    redirected
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]",
                        )
                        .await
                        .unwrap();
                    true
                }
                _ => false,
            }
        });

        let client = super::BinanceFuturesClient::new(
            &format!("http://{address}"),
            "sensitive-key",
            "sensitive-secret",
        )
        .unwrap();
        let _ = client.get_position_information().await;

        assert!(!server.await.unwrap(), "signed request followed a redirect");
    }

    #[tokio::test]
    async fn request_timeout_bounds_a_server_that_never_responds() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            time::sleep(Duration::from_secs(1)).await;
        });
        let client = super::BinanceFuturesClient::new_with_timeout(
            &format!("http://{address}"),
            "key",
            "secret",
            Duration::from_millis(50),
        )
        .unwrap();

        let started = std::time::Instant::now();
        let result = client.get_position_information().await;

        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "request timeout must bound an accepted connection with no response"
        );
        server.abort();
    }
}
