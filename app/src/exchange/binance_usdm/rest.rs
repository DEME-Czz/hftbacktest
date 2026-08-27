use std::{fmt::Write, time::Duration};

use chrono::Utc;
use hftbacktest::types::{OrdType, Side, TimeInForce};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use tracing::warn;

use super::{
    BinanceFuturesError,
    protocol::{
        rest::{self, ErrorResponse, OrderResponse, PositionInformationV3},
        stream::ListenKey,
    },
};

const AMBIGUOUS_ORDER_RESPONSE_CODE: i64 = -1_000_001;

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

fn unwrap_order_payload(value: serde_json::Value) -> serde_json::Value {
    if let Some(error) = value.get("error") {
        error.clone()
    } else if let Some(result) = value.get("result") {
        result.clone()
    } else {
        value
    }
}

fn parse_order_response_value(
    value: serde_json::Value,
) -> Result<OrderResponse, BinanceFuturesError> {
    let payload = unwrap_order_payload(value);
    if let Ok(error) = serde_json::from_value::<ErrorResponse>(payload.clone()) {
        return Err(BinanceFuturesError::OrderError {
            code: error.code,
            msg: error.msg,
        });
    }

    serde_json::from_value::<OrderResponse>(payload.clone()).map_err(|error| {
        warn!(
            ?error,
            response = ?payload,
            "couldn't decode Binance order response"
        );
        BinanceFuturesError::OrderError {
            code: AMBIGUOUS_ORDER_RESPONSE_CODE,
            msg: "unrecognized Binance order response schema".to_string(),
        }
    })
}

fn parse_query_order_value(
    value: serde_json::Value,
) -> Result<Option<OrderResponse>, BinanceFuturesError> {
    let payload = unwrap_order_payload(value);

    if payload.is_null() {
        return Ok(None);
    }

    if let Ok(error) = serde_json::from_value::<ErrorResponse>(payload.clone()) {
        return if error.code == -2013 {
            Ok(None)
        } else {
            Err(BinanceFuturesError::OrderError {
                code: error.code,
                msg: error.msg,
            })
        };
    }

    match serde_json::from_value::<OrderResponse>(payload.clone()) {
        Ok(order) => Ok(Some(order)),
        Err(error) => {
            warn!(
                ?error,
                response = ?payload,
                "couldn't decode Binance order query response; deferring to account reconciliation"
            );
            Ok(None)
        }
    }
}

fn parse_open_orders_value(
    value: serde_json::Value,
) -> Result<Vec<OrderResponse>, BinanceFuturesError> {
    let payload = unwrap_order_payload(value);

    if let Ok(error) = serde_json::from_value::<ErrorResponse>(payload.clone()) {
        return Err(BinanceFuturesError::OrderError {
            code: error.code,
            msg: error.msg,
        });
    }

    serde_json::from_value::<Vec<OrderResponse>>(payload.clone()).map_err(|error| {
        warn!(
            ?error,
            response = ?payload,
            "couldn't decode Binance open orders response"
        );
        BinanceFuturesError::InvalidAccountState
    })
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
        Self::new_with_timeout(url, api_key, secret, Duration::from_secs(5))
    }

    fn new_with_timeout(
        url: &str,
        api_key: &str,
        secret: &str,
        request_timeout: Duration,
    ) -> Result<Self, BinanceFuturesError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(request_timeout)
                .timeout(request_timeout)
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

    async fn resolve_ambiguous_cancel(
        &self,
        client_order_id: &str,
        symbol: &str,
        reason: &str,
    ) -> Result<OrderResponse, BinanceFuturesError> {
        match self.query_order(client_order_id, symbol).await {
            Ok(Some(order)) => Ok(order),
            Ok(None) => Err(BinanceFuturesError::OrderError {
                code: -2011,
                msg: format!("{reason}; account reconciliation required"),
            }),
            Err(error) => {
                warn!(
                    %symbol,
                    %client_order_id,
                    ?error,
                    "cancel verification failed; deferring to account reconciliation"
                );
                Err(BinanceFuturesError::OrderError {
                    code: -2011,
                    msg: format!("{reason}; verification failed; account reconciliation required"),
                })
            }
        }
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

        let response: serde_json::Value = self.post("/fapi/v1/order", body).await?;
        parse_order_response_value(response)
    }

    pub async fn cancel_order(
        &self,
        client_order_id: &str,
        symbol: &str,
    ) -> Result<OrderResponse, BinanceFuturesError> {
        let body = format!("symbol={symbol}&origClientOrderId={client_order_id}");
        let response: serde_json::Value = match self.delete("/fapi/v1/order", body).await {
            Ok(response) => response,
            Err(BinanceFuturesError::ReqError(error)) => {
                warn!(
                    %symbol,
                    %client_order_id,
                    ?error,
                    "cancel request outcome is ambiguous; verifying order state"
                );
                return self
                    .resolve_ambiguous_cancel(client_order_id, symbol, "cancel request failed")
                    .await;
            }
            Err(error) => return Err(error),
        };

        match parse_order_response_value(response) {
            Ok(order) => Ok(order),
            Err(BinanceFuturesError::OrderError { code, .. })
                if code == AMBIGUOUS_ORDER_RESPONSE_CODE =>
            {
                self.resolve_ambiguous_cancel(
                    client_order_id,
                    symbol,
                    "cancel response schema was not recognized",
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn query_order(
        &self,
        client_order_id: &str,
        symbol: &str,
    ) -> Result<Option<OrderResponse>, BinanceFuturesError> {
        let query = format!("symbol={symbol}&origClientOrderId={client_order_id}");
        let response: serde_json::Value = match self.get("/fapi/v1/order", query).await {
            Ok(response) => response,
            Err(BinanceFuturesError::ReqError(error))
                if error.is_timeout() || error.is_decode() =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        parse_query_order_value(response)
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
        let response: serde_json::Value = self
            .get("/fapi/v1/openOrders", format!("symbol={symbol}"))
            .await?;
        parse_open_orders_value(response)
    }

    pub async fn get_depth(&self, symbol: &str) -> Result<rest::Depth, BinanceFuturesError> {
        self.get_noauth("/fapi/v1/depth", format!("symbol={symbol}&limit=1000"))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        AMBIGUOUS_ORDER_RESPONSE_CODE, parse_open_orders_value, parse_order_response_value,
        parse_query_order_value, side_str, sign_hmac_sha256,
    };
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

    #[test]
    fn open_orders_parser_accepts_direct_array() {
        let value = serde_json::json!([]);
        assert!(parse_open_orders_value(value).unwrap().is_empty());
    }

    #[test]
    fn open_orders_parser_accepts_result_envelope() {
        let value = serde_json::json!({"result": []});
        assert!(parse_open_orders_value(value).unwrap().is_empty());
    }

    #[test]
    fn open_orders_parser_preserves_binance_error() {
        let error = parse_open_orders_value(serde_json::json!({
            "error": {"code": -2015, "msg": "Rejected"}
        }))
        .unwrap_err();
        assert!(matches!(
            error,
            crate::exchange::binance_usdm::BinanceFuturesError::OrderError { code: -2015, .. }
        ));
    }

    #[test]
    fn order_response_parser_rejects_unknown_payload() {
        let error = parse_order_response_value(serde_json::json!({"unexpected": true})).unwrap_err();
        assert!(matches!(
            error,
            crate::exchange::binance_usdm::BinanceFuturesError::OrderError {
                code: AMBIGUOUS_ORDER_RESPONSE_CODE,
                ..
            }
        ));
    }

    #[test]
    fn query_order_parser_treats_unknown_payload_as_unresolved() {
        assert!(
            parse_query_order_value(serde_json::json!({"unexpected": true}))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn order_response_parser_accepts_result_envelope() {
        let value = serde_json::json!({
            "result": {
                "clientOrderId": "cid",
                "cumQty": "0",
                "executedQty": "0",
                "origQty": "0.001",
                "price": "100",
                "side": "BUY",
                "status": "NEW",
                "symbol": "BTCUSDT",
                "timeInForce": "GTC",
                "type": "LIMIT",
                "updateTime": 1
            }
        });
        let order = parse_order_response_value(value).unwrap();
        assert_eq!(order.client_order_id, "cid");
        assert_eq!(order.symbol, "btcusdt");
    }

    #[test]
    fn query_order_parser_accepts_result_envelope() {
        let value = serde_json::json!({
            "result": {
                "clientOrderId": "cid",
                "cumQty": "0",
                "executedQty": "0",
                "origQty": "0.001",
                "price": "100",
                "side": "BUY",
                "status": "NEW",
                "symbol": "BTCUSDT",
                "timeInForce": "GTC",
                "type": "LIMIT",
                "updateTime": 1
            }
        });
        assert!(parse_query_order_value(value).unwrap().is_some());
    }

    #[test]
    fn order_response_parser_accepts_missing_optional_fields() {
        let value = serde_json::json!({
            "clientOrderId": "cid",
            "cumQty": "0",
            "executedQty": "0",
            "origQty": "0.001",
            "price": "100",
            "side": "BUY",
            "status": "NEW",
            "symbol": "BTCUSDT",
            "type": "LIMIT"
        });
        let order = parse_order_response_value(value).unwrap();
        assert_eq!(order.client_order_id, "cid");
        assert_eq!(
            order.time_in_force,
            hftbacktest::types::TimeInForce::Unsupported
        );
        assert_eq!(order.update_time, 0);
    }

    #[test]
    fn query_order_parser_accepts_missing_optional_fields() {
        let value = serde_json::json!({
            "clientOrderId": "cid",
            "cumQty": "0",
            "executedQty": "0",
            "origQty": "0.001",
            "price": "100",
            "side": "BUY",
            "status": "NEW",
            "symbol": "BTCUSDT",
            "type": "LIMIT"
        });
        assert!(parse_query_order_value(value).unwrap().is_some());
    }

    #[test]
    fn order_response_parser_preserves_binance_error() {
        let error = parse_order_response_value(serde_json::json!({
            "error": {"code": -2011, "msg": "Unknown order sent."}
        }))
        .unwrap_err();
        assert!(matches!(
            error,
            crate::exchange::binance_usdm::BinanceFuturesError::OrderError { code: -2011, .. }
        ));
    }

    #[test]
    fn query_order_parser_maps_unknown_order_to_none() {
        let result = parse_query_order_value(serde_json::json!({
            "error": {"code": -2013, "msg": "Order does not exist."}
        }))
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn order_response_parser_accepts_direct_payload() {
        let value = serde_json::json!({
            "clientOrderId": "cid",
            "cumQty": "0",
            "executedQty": "0",
            "origQty": "0.001",
            "price": "100",
            "side": "BUY",
            "status": "NEW",
            "symbol": "BTCUSDT",
            "timeInForce": "GTC",
            "type": "LIMIT",
            "updateTime": 1
        });
        let order = parse_order_response_value(value).unwrap();
        assert_eq!(order.client_order_id, "cid");
        assert_eq!(order.symbol, "btcusdt");
    }

    #[tokio::test]
    async fn query_order_timeout_returns_none() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = socket.read(&mut buffer).await.unwrap();
            time::sleep(Duration::from_millis(200)).await;
        });

        let client = super::BinanceFuturesClient::new_with_timeout(
            &format!("http://{address}"),
            "key",
            "secret",
            Duration::from_millis(50),
        )
        .unwrap();
        assert!(client.query_order("cid", "btcusdt").await.unwrap().is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn query_order_decode_error_returns_none() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = socket.read(&mut buffer).await.unwrap();
            let body = b"not-json";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
        });

        let client = super::BinanceFuturesClient::new_with_timeout(
            &format!("http://{address}"),
            "key",
            "secret",
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(client.query_order("cid", "btcusdt").await.unwrap().is_none());
        server.await.unwrap();
    }
}
