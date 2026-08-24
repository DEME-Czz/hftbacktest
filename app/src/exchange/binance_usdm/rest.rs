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

#[derive(Clone)]
pub struct BinanceFuturesClient {
    client: reqwest::Client,
    url: String,
    api_key: String,
    secret: String,
}

impl BinanceFuturesClient {
    pub fn new(url: &str, api_key: &str, secret: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            secret: secret.to_string(),
        }
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
        price_precision: usize,
        quantity: f64,
        order_type: OrdType,
        time_in_force: TimeInForce,
    ) -> Result<OrderResponse, BinanceFuturesError> {
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
        body.push_str(&format!("{quantity:.5}"));
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
    use super::{side_str, sign_hmac_sha256};
    use hftbacktest::types::Side;

    #[test]
    fn unsupported_order_side_is_rejected_without_panicking() {
        assert!(side_str(Side::None).is_err());
        assert!(side_str(Side::Unsupported).is_err());
    }

    #[tokio::test]
    async fn unsupported_order_shape_is_rejected_without_an_http_request() {
        use hftbacktest::types::{OrdType, TimeInForce};

        let client = super::BinanceFuturesClient::new("http://127.0.0.1:9", "key", "secret");
        let unsupported_type = client
            .submit_order(
                "client-id",
                "BTCUSDT",
                Side::Buy,
                100.0,
                1,
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
                1,
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
}
