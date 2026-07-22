use chrono::Utc;
use hftbacktest::types::{OrdType, Side, TimeInForce};
use serde::Deserialize;
use std::time::Duration;

use super::msg::{
    rest,
    rest::{AccountInformationV3, PositionInformationV3},
};
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
    pub fn new(
        url: &str,
        api_key: &str,
        secret: &str,
        proxy_url: Option<&str>,
    ) -> Result<Self, reqwest::Error> {
        let mut client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15));
        if let Some(proxy_url) = proxy_url {
            client = client.proxy(reqwest::Proxy::all(proxy_url)?);
        } else {
            client = client.no_proxy();
        }
        let client = client.build()?;
        Ok(Self {
            client,
            url: url.to_string(),
            api_key: api_key.to_string(),
            secret: secret.to_string(),
        })
    }

    async fn get_noauth<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        query: String,
    ) -> Result<T, reqwest::Error> {
        let resp = self
            .client
            .get(format!("{}{}?{}", self.url, path, query))
            .header("Accept", "application/json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(resp)
    }

    async fn get<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        mut query: String,
    ) -> Result<T, reqwest::Error> {
        let time = Utc::now().timestamp_millis() - 1000;
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str("recvWindow=5000&timestamp=");
        query.push_str(&time.to_string());
        let signature = sign_hmac_sha256(&self.secret, &query);
        let resp = self
            .client
            .get(format!(
                "{}{}?{}&signature={}",
                self.url, path, query, signature
            ))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(resp)
    }

    async fn put<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        body: String,
    ) -> Result<T, reqwest::Error> {
        let time = Utc::now().timestamp_millis() - 1000;
        let sign_body = format!("recvWindow=5000&timestamp={time}{body}");
        let signature = sign_hmac_sha256(&self.secret, &sign_body);
        let resp = self
            .client
            .put(format!(
                "{}{}?recvWindow=5000&timestamp={}&signature={}",
                self.url, path, time, signature
            ))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .body(body)
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    async fn post<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        body: String,
    ) -> Result<T, reqwest::Error> {
        let time = Utc::now().timestamp_millis() - 1000;
        let sign_body = format!("recvWindow=5000&timestamp={time}{body}");
        let signature = sign_hmac_sha256(&self.secret, &sign_body);
        let resp = self
            .client
            .post(format!(
                "{}{}?recvWindow=5000&timestamp={}&signature={}",
                self.url, path, time, signature
            ))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .body(body)
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    async fn delete<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        body: String,
    ) -> Result<T, reqwest::Error> {
        let time = Utc::now().timestamp_millis() - 1000;
        let sign_body = format!("recvWindow=5000&timestamp={time}{body}");
        let signature = sign_hmac_sha256(&self.secret, &sign_body);
        let resp = self
            .client
            .delete(format!(
                "{}{}?recvWindow=5000&timestamp={}&signature={}",
                self.url, path, time, signature
            ))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .body(body)
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    async fn delete_query<T: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        mut params: Vec<(&str, String)>,
    ) -> Result<T, reqwest::Error> {
        let timestamp = (Utc::now().timestamp_millis() - 1000).to_string();
        params.push(("recvWindow", "5000".to_string()));
        params.push(("timestamp", timestamp));

        let mut request = self
            .client
            .delete(format!("{}{}", self.url, path))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .query(&params)
            .build()?;
        let signature = sign_hmac_sha256(&self.secret, request.url().query().unwrap_or_default());
        request
            .url_mut()
            .query_pairs_mut()
            .append_pair("signature", &signature);

        self.client.execute(request).await?.json().await
    }

    pub async fn start_user_data_stream(&self) -> Result<String, reqwest::Error> {
        let resp: ListenKey = self
            .client
            .post(format!("{}/fapi/v1/listenKey", self.url))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(resp.listen_key)
    }

    pub async fn keepalive_user_data_stream(&self) -> Result<(), reqwest::Error> {
        self.client
            .put(format!("{}/fapi/v1/listenKey", self.url))
            .header("Accept", "application/json")
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn submit_order(
        &self,
        client_order_id: &str,
        symbol: &str,
        side: Side,
        price: f64,
        price_prec: usize,
        qty: f64,
        order_type: OrdType,
        time_in_force: TimeInForce,
    ) -> Result<OrderResponse, BinanceFuturesError> {
        let mut body = String::with_capacity(200);
        body.push_str("newClientOrderId=");
        body.push_str(client_order_id);
        body.push_str("&symbol=");
        body.push_str(symbol);
        body.push_str("&side=");
        body.push_str(side.as_ref());
        body.push_str("&price=");
        body.push_str(&format!("{price:.price_prec$}"));
        body.push_str("&quantity=");
        body.push_str(&format!("{qty:.5}"));
        body.push_str("&type=");
        body.push_str(order_type.as_ref());
        body.push_str("&timeInForce=");
        body.push_str(time_in_force.as_ref());

        let resp: OrderResponseResult = self.post("/fapi/v1/order", body).await?;
        match resp {
            OrderResponseResult::Ok(resp) => Ok(resp),
            OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError {
                code: resp.code,
                msg: resp.msg,
            }),
        }
    }

    pub async fn submit_orders(
        &self,
        orders: Vec<(String, String, Side, f64, usize, f64, OrdType, TimeInForce)>,
    ) -> Result<Vec<Result<OrderResponse, BinanceFuturesError>>, BinanceFuturesError> {
        if orders.len() > 5 {
            return Err(BinanceFuturesError::InvalidRequest);
        }
        let mut body = String::with_capacity(2000 * orders.len());
        body.push_str("{\"batchOrders\":[");
        for (i, order) in orders.iter().enumerate() {
            if i > 0 {
                body.push(',');
            }
            body.push_str("{\"newClientOrderId\":\"");
            body.push_str(&order.0);
            body.push_str("\",\"symbol\":\"");
            body.push_str(&order.1);
            body.push_str("\",\"side\":\"");
            body.push_str(order.2.as_ref());
            body.push_str("\",\"price\":\"");
            body.push_str(&format!("{:.prec$}", order.3, prec = order.4));
            body.push_str("\",\"quantity\":\"");
            body.push_str(&format!("{:.5}", order.5));
            body.push_str("\",\"type\":\"");
            body.push_str(order.6.as_ref());
            body.push_str("\",\"timeInForce\":\"");
            body.push_str(order.7.as_ref());
            body.push_str("\"}");
        }
        body.push_str("]}");

        let resp: Vec<OrderResponseResult> = self.post("/fapi/v1/batchOrders", body).await?;
        Ok(resp
            .into_iter()
            .map(|resp| match resp {
                OrderResponseResult::Ok(resp) => Ok(resp),
                OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError {
                    code: resp.code,
                    msg: resp.msg,
                }),
            })
            .collect())
    }

    pub async fn modify_order(
        &self,
        client_order_id: &str,
        symbol: &str,
        side: Side,
        price: f64,
        price_prec: usize,
        qty: f64,
    ) -> Result<OrderResponse, BinanceFuturesError> {
        let mut body = String::with_capacity(100);
        body.push_str("symbol=");
        body.push_str(symbol);
        body.push_str("&origClientOrderId=");
        body.push_str(client_order_id);
        body.push_str("&side=");
        body.push_str(side.as_ref());
        body.push_str("&price=");
        body.push_str(&format!("{price:.price_prec$}"));
        body.push_str("&quantity=");
        body.push_str(&format!("{qty:.5}"));

        let resp: OrderResponseResult = self.put("/fapi/v1/order", body).await?;
        match resp {
            OrderResponseResult::Ok(resp) => Ok(resp),
            OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError {
                code: resp.code,
                msg: resp.msg,
            }),
        }
    }

    pub async fn cancel_order(
        &self,
        client_order_id: &str,
        symbol: &str,
    ) -> Result<OrderResponse, BinanceFuturesError> {
        let mut body = String::with_capacity(100);
        body.push_str("symbol=");
        body.push_str(symbol);
        body.push_str("&origClientOrderId=");
        body.push_str(client_order_id);

        let resp: OrderResponseResult = self.delete("/fapi/v1/order", body).await?;
        match resp {
            OrderResponseResult::Ok(resp) => Ok(resp),
            OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError {
                code: resp.code,
                msg: resp.msg,
            }),
        }
    }

    pub async fn cancel_orders(
        &self,
        symbol: &str,
        client_order_ids: Vec<String>,
    ) -> Result<Vec<Result<OrderResponse, BinanceFuturesError>>, BinanceFuturesError> {
        if client_order_ids.len() > 10 {
            return Err(BinanceFuturesError::InvalidRequest);
        }
        let client_order_ids = serde_json::to_string(&client_order_ids)
            .map_err(|_| BinanceFuturesError::InvalidRequest)?;
        let resp: Vec<OrderResponseResult> = self
            .delete_query(
                "/fapi/v1/batchOrders",
                vec![
                    ("symbol", symbol.to_string()),
                    ("origClientOrderIdList", client_order_ids),
                ],
            )
            .await?;
        Ok(resp
            .into_iter()
            .map(|resp| match resp {
                OrderResponseResult::Ok(resp) => Ok(resp),
                OrderResponseResult::Err(resp) => Err(BinanceFuturesError::OrderError {
                    code: resp.code,
                    msg: resp.msg,
                }),
            })
            .collect())
    }

    pub async fn cancel_all_orders(&self, symbol: &str) -> Result<(), reqwest::Error> {
        let _: serde_json::Value = self
            .delete("/fapi/v1/allOpenOrders", format!("symbol={symbol}"))
            .await?;
        Ok(())
    }

    pub async fn get_position_information(
        &self,
    ) -> Result<Vec<PositionInformationV3>, reqwest::Error> {
        let resp: Vec<PositionInformationV3> =
            self.get("/fapi/v3/positionRisk", String::new()).await?;
        Ok(resp)
    }

    pub async fn get_account_information(&self) -> Result<AccountInformationV3, reqwest::Error> {
        self.get("/fapi/v3/account", String::new()).await
    }

    pub async fn get_depth(&self, symbol: &str) -> Result<rest::Depth, reqwest::Error> {
        let resp: rest::Depth = self
            .get_noauth("/fapi/v1/depth", format!("symbol={symbol}&limit=1000"))
            .await?;
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::BinanceFuturesClient;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn serve_once(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test server address");
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer).expect("read test request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(response.as_bytes())
                .expect("write test response");
            String::from_utf8(request).expect("request is UTF-8")
        });
        (format!("http://{address}"), handle)
    }

    #[tokio::test]
    async fn listen_key_preserves_http_error_status() {
        let (url, server) = serve_once(
            "418 I'm a teapot",
            r#"{"code":-1003,"msg":"Way too much request weight used; IP banned."}"#,
        );
        let client =
            BinanceFuturesClient::new(&url, "api-key", "secret", None).expect("build REST client");

        let error = client
            .start_user_data_stream()
            .await
            .expect_err("HTTP 418 must fail before JSON decoding");

        assert_eq!(error.status().map(|status| status.as_u16()), Some(418));
        server.join().expect("join test server");
    }

    #[tokio::test]
    async fn listen_key_uses_api_key_auth_without_signature() {
        let (url, server) = serve_once("200 OK", r#"{"listenKey":"test-listen-key"}"#);
        let client =
            BinanceFuturesClient::new(&url, "api-key", "secret", None).expect("build REST client");

        let listen_key = client
            .start_user_data_stream()
            .await
            .expect("parse listen key");
        let request = server.join().expect("join test server");

        assert_eq!(listen_key, "test-listen-key");
        assert!(request.starts_with("POST /fapi/v1/listenKey HTTP/1.1\r\n"));
        assert!(request.contains("x-mbx-apikey: api-key\r\n"));
        assert!(!request.contains("timestamp="));
        assert!(!request.contains("signature="));
    }
}
