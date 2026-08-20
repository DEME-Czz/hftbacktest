use chrono::Utc;
use hftbacktest::types::{OrdType, Side, TimeInForce};
use serde::Deserialize;

use super::msg::{rest, rest::PositionInformationV3};
use crate::{
    binancefutures::{
        BinanceFuturesError,
        msg::{
            rest::{OrderResponse, OrderResponseResult},
            stream::ListenKey,
        },
    },
    utils::sign_hmac_sha256,
};

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
            url: url.to_string(),
            api_key: api_key.to_string(),
            secret: secret.to_string(),
        }
    }

    async fn get_noauth<T: for<'a> Deserialize<'a>>(&self, path: &str, query: String) -> Result<T, reqwest::Error> {
        self.client.get(format!("{}{}?{}", self.url, path, query)).header("Accept", "application/json").send().await?.json().await
    }

    fn signed_query(&self, query: &str) -> String {
        let timestamp = Utc::now().timestamp_millis();
        if query.is_empty() { format!("recvWindow=5000&timestamp={timestamp}") } else { format!("{query}&recvWindow=5000&timestamp={timestamp}") }
    }

    async fn get<T: for<'a> Deserialize<'a>>(&self, path: &str, query: String) -> Result<T, reqwest::Error> {
        let signed_query = self.signed_query(&query);
        let signature = sign_hmac_sha256(&self.secret, &signed_query);
        self.client.get(format!("{}{}?{}&signature={}", self.url, path, signed_query, signature))
            .header("Accept", "application/json").header("X-MBX-APIKEY", &self.api_key).send().await?.json().await
    }

    async fn signed_body_request<T: for<'a> Deserialize<'a>>(&self, method: reqwest::Method, path: &str, body: String) -> Result<T, reqwest::Error> {
        let timestamp = Utc::now().timestamp_millis();
        let mut params = body;
        if !params.is_empty() { params.push('&'); }
        params.push_str(&format!("recvWindow=5000&timestamp={timestamp}"));
        let signature = sign_hmac_sha256(&self.secret, &params);
        params.push_str("&signature=");
        params.push_str(&signature);
        self.client.request(method, format!("{}{}", self.url, path))
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("X-MBX-APIKEY", &self.api_key)
            .body(params).send().await?.json().await
    }

    async fn put<T: for<'a> Deserialize<'a>>(&self, path: &str, body: String) -> Result<T, reqwest::Error> {
        self.signed_body_request(reqwest::Method::PUT, path, body).await
    }
    async fn post<T: for<'a> Deserialize<'a>>(&self, path: &str, body: String) -> Result<T, reqwest::Error> {
        self.signed_body_request(reqwest::Method::POST, path, body).await
    }
    async fn delete<T: for<'a> Deserialize<'a>>(&self, path: &str, body: String) -> Result<T, reqwest::Error> {
        self.signed_body_request(reqwest::Method::DELETE, path, body).await
    }

    async fn user_stream_request<T: for<'a> Deserialize<'a>>(&self, method: reqwest::Method) -> Result<T, reqwest::Error> {
        self.client.request(method, format!("{}/fapi/v1/listenKey", self.url))
            .header("Accept", "application/json").header("X-MBX-APIKEY", &self.api_key).send().await?.json().await
    }

    pub async fn start_user_data_stream(&self) -> Result<String, reqwest::Error> {
        let resp: ListenKey = self.user_stream_request(reqwest::Method::POST).await?;
        Ok(resp.listen_key)
    }
    pub async fn keepalive_user_data_stream(&self) -> Result<String, reqwest::Error> {
        let resp: ListenKey = self.user_stream_request(reqwest::Method::PUT).await?;
        Ok(resp.listen_key)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn submit_order(&self, client_order_id: &str, symbol: &str, side: Side, price: f64, price_prec: usize, qty: f64, order_type: OrdType, time_in_force: TimeInForce) -> Result<OrderResponse, BinanceFuturesError> {
        let mut body = String::with_capacity(220);
        body.push_str("newClientOrderId="); body.push_str(client_order_id);
        body.push_str("&symbol="); body.push_str(symbol);
        body.push_str("&side="); body.push_str(side.as_ref());
        if order_type == OrdType::Limit {
            body.push_str("&price="); body.push_str(&format!("{price:.price_prec$}"));
            body.push_str("&timeInForce="); body.push_str(time_in_force.as_ref());
        }
        body.push_str("&quantity="); body.push_str(&format!("{qty:.5}"));
        body.push_str("&type="); body.push_str(order_type.as_ref());
        body.push_str("&newOrderRespType=RESULT");
        let resp: OrderResponseResult = self.post("/fapi/v1/order", body).await?;
        match resp { OrderResponseResult::Ok(resp) => Ok(resp), OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError { code: resp.code, msg: resp.msg }) }
    }

    pub async fn submit_orders(&self, orders: Vec<(String, String, Side, f64, usize, f64, OrdType, TimeInForce)>) -> Result<Vec<Result<OrderResponse, BinanceFuturesError>>, BinanceFuturesError> {
        if orders.len() > 5 { return Err(BinanceFuturesError::InvalidRequest); }
        let mut body = String::with_capacity(2000 * orders.len());
        body.push_str("batchOrders=[");
        for (i, order) in orders.iter().enumerate() {
            if i > 0 { body.push(','); }
            body.push_str("{\"newClientOrderId\":\""); body.push_str(&order.0);
            body.push_str("\",\"symbol\":\""); body.push_str(&order.1);
            body.push_str("\",\"side\":\""); body.push_str(order.2.as_ref());
            if order.6 == OrdType::Limit {
                body.push_str("\",\"price\":\""); body.push_str(&format!("{:.prec$}", order.3, prec = order.4));
                body.push_str("\",\"timeInForce\":\""); body.push_str(order.7.as_ref());
            }
            body.push_str("\",\"quantity\":\""); body.push_str(&format!("{:.5}", order.5));
            body.push_str("\",\"type\":\""); body.push_str(order.6.as_ref());
            body.push_str("\",\"newOrderRespType\":\"RESULT\"}");
        }
        body.push(']');
        let resp: Vec<OrderResponseResult> = self.post("/fapi/v1/batchOrders", body).await?;
        Ok(resp.into_iter().map(|resp| match resp { OrderResponseResult::Ok(resp) => Ok(resp), OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError { code: resp.code, msg: resp.msg }) }).collect())
    }

    pub async fn modify_order(&self, client_order_id: &str, symbol: &str, side: Side, price: f64, price_prec: usize, qty: f64) -> Result<OrderResponse, BinanceFuturesError> {
        let body = format!("symbol={symbol}&origClientOrderId={client_order_id}&side={}&price={price:.price_prec$}&quantity={qty:.5}", side.as_ref());
        let resp: OrderResponseResult = self.put("/fapi/v1/order", body).await?;
        match resp { OrderResponseResult::Ok(resp) => Ok(resp), OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError { code: resp.code, msg: resp.msg }) }
    }

    pub async fn cancel_order(&self, client_order_id: &str, symbol: &str) -> Result<OrderResponse, BinanceFuturesError> {
        let body = format!("symbol={symbol}&origClientOrderId={client_order_id}");
        let resp: OrderResponseResult = self.delete("/fapi/v1/order", body).await?;
        match resp { OrderResponseResult::Ok(resp) => Ok(resp), OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError { code: resp.code, msg: resp.msg }) }
    }

    pub async fn cancel_orders(&self, symbol: &str, client_order_ids: Vec<String>) -> Result<Vec<Result<OrderResponse, BinanceFuturesError>>, BinanceFuturesError> {
        if client_order_ids.len() > 10 { return Err(BinanceFuturesError::InvalidRequest); }
        let ids = serde_json::to_string(&client_order_ids).map_err(|_| BinanceFuturesError::InvalidRequest)?;
        let body = format!("symbol={symbol}&origClientOrderIdList={ids}");
        let resp: Vec<OrderResponseResult> = self.delete("/fapi/v1/batchOrders", body).await?;
        Ok(resp.into_iter().map(|resp| match resp { OrderResponseResult::Ok(resp) => Ok(resp), OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError { code: resp.code, msg: resp.msg }) }).collect())
    }

    pub async fn cancel_all_orders(&self, symbol: &str) -> Result<(), reqwest::Error> {
        let _: serde_json::Value = self.delete("/fapi/v1/allOpenOrders", format!("symbol={symbol}" )).await?;
        Ok(())
    }
    pub async fn get_position_information(&self) -> Result<Vec<PositionInformationV3>, reqwest::Error> {
        self.get("/fapi/v3/positionRisk", String::new()).await
    }
    pub async fn get_depth(&self, symbol: &str) -> Result<rest::Depth, reqwest::Error> {
        self.get_noauth("/fapi/v1/depth", format!("symbol={symbol}&limit=1000")).await
    }
}
