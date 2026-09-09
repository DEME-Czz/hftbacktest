use hashbrown::Equivalent;
use hftbacktest::prelude::OrderId;
use rand::Rng;

const CLIENT_ORDER_ID_MAX_LEN: usize = 36;
const CLIENT_ORDER_ID_NONCE_LEN: usize = 6;
const CLIENT_ORDER_ID_VERSION: &str = "v1";
const ORDER_ID_BASE: u32 = 36;
const MAX_PREFIX_LEN: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClientOrderIdError {
    InvalidPrefix,
    Malformed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClientOrderIdCodec {
    prefix: String,
}

impl ClientOrderIdCodec {
    pub(crate) fn new(prefix: &str) -> Result<Self, ClientOrderIdError> {
        if prefix.is_empty()
            || prefix.len() > MAX_PREFIX_LEN
            || !prefix.bytes().all(is_allowed_client_order_id_byte)
        {
            return Err(ClientOrderIdError::InvalidPrefix);
        }
        Ok(Self {
            prefix: prefix.to_string(),
        })
    }

    pub(crate) fn encode(&self, order_id: OrderId) -> String {
        let encoded_order_id = encode_base36(order_id);
        let nonce = generate_random_id(CLIENT_ORDER_ID_NONCE_LEN);
        let client_order_id = format!(
            "{}-{}-{}-{}",
            self.prefix, CLIENT_ORDER_ID_VERSION, encoded_order_id, nonce
        );
        debug_assert!(client_order_id.len() <= CLIENT_ORDER_ID_MAX_LEN);
        client_order_id
    }

    /// Returns `Ok(None)` for another strategy's namespace and an error for an identifier that
    /// starts with this strategy's prefix but cannot safely be recovered.
    pub(crate) fn decode(
        &self,
        client_order_id: &str,
    ) -> Result<Option<OrderId>, ClientOrderIdError> {
        let owned_marker = format!("{}-{}-", self.prefix, CLIENT_ORDER_ID_VERSION);
        if !client_order_id.starts_with(&owned_marker) {
            return if client_order_id.starts_with(&self.prefix) {
                Err(ClientOrderIdError::Malformed)
            } else {
                Ok(None)
            };
        }
        if client_order_id.len() > CLIENT_ORDER_ID_MAX_LEN
            || !client_order_id.bytes().all(is_allowed_client_order_id_byte)
        {
            return Err(ClientOrderIdError::Malformed);
        }

        let payload = &client_order_id[owned_marker.len()..];
        let Some((encoded_order_id, nonce)) = payload.split_once('-') else {
            return Err(ClientOrderIdError::Malformed);
        };
        if encoded_order_id.is_empty()
            || encoded_order_id.contains('-')
            || nonce.len() != CLIENT_ORDER_ID_NONCE_LEN
            || !nonce.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(ClientOrderIdError::Malformed);
        }
        let order_id = OrderId::from_str_radix(encoded_order_id, ORDER_ID_BASE)
            .map_err(|_| ClientOrderIdError::Malformed)?;
        Ok(Some(order_id))
    }
}

fn is_allowed_client_order_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
}

fn encode_base36(mut value: OrderId) -> String {
    const DIGITS: &[u8; ORDER_ID_BASE as usize] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }

    let mut encoded = [0_u8; 13];
    let mut cursor = encoded.len();
    while value > 0 {
        cursor -= 1;
        encoded[cursor] = DIGITS[(value % u64::from(ORDER_ID_BASE)) as usize];
        value /= u64::from(ORDER_ID_BASE);
    }
    String::from_utf8_lossy(&encoded[cursor..]).into_owned()
}

#[derive(Clone, Eq, Hash, PartialEq, Debug)]
pub struct SymbolOrderId {
    pub symbol: String,
    pub order_id: OrderId,
}

impl SymbolOrderId {
    pub fn new(symbol: String, order_id: OrderId) -> Self {
        Self { symbol, order_id }
    }
}

#[derive(Eq, Hash, PartialEq, Debug)]
pub struct RefSymbolOrderId<'a> {
    pub symbol: &'a str,
    pub order_id: OrderId,
}

impl<'a> RefSymbolOrderId<'a> {
    pub fn new(symbol: &'a str, order_id: OrderId) -> Self {
        Self { symbol, order_id }
    }
}

impl Equivalent<SymbolOrderId> for RefSymbolOrderId<'_> {
    fn equivalent(&self, key: &SymbolOrderId) -> bool {
        key.symbol == self.symbol && key.order_id == self.order_id
    }
}

pub fn generate_random_id(length: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ClientOrderIdCodec, ClientOrderIdError};

    #[test]
    fn client_order_id_round_trips_local_order_id_within_exchange_limit() {
        let codec = ClientOrderIdCodec::new("strategy-a").unwrap();

        for order_id in [0, 1, 35, 36, u64::MAX] {
            let encoded = codec.encode(order_id);
            assert!(encoded.len() <= 36, "{encoded} exceeds Binance's limit");
            assert!(
                encoded.bytes().all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')),
                "{encoded} contains an exchange-invalid character"
            );
            assert_eq!(codec.decode(&encoded).unwrap(), Some(order_id));
        }
    }

    #[test]
    fn client_order_id_distinguishes_foreign_from_malformed_owned_ids() {
        let codec = ClientOrderIdCodec::new("strategy-a").unwrap();

        assert_eq!(codec.decode("other-v1-1-AbCd12").unwrap(), None);
        assert_eq!(
            codec.decode("strategy-a-v1-not-base36-AbCd12"),
            Err(ClientOrderIdError::Malformed)
        );
        assert_eq!(
            codec.decode("strategy-a-v1-1-bad!id"),
            Err(ClientOrderIdError::Malformed)
        );
    }

    #[test]
    fn client_order_id_rejects_unsafe_or_oversized_prefixes() {
        assert_eq!(
            ClientOrderIdCodec::new(""),
            Err(ClientOrderIdError::InvalidPrefix)
        );
        assert_eq!(
            ClientOrderIdCodec::new("contains space"),
            Err(ClientOrderIdError::InvalidPrefix)
        );
        assert_eq!(
            ClientOrderIdCodec::new("prefix-is-too-long"),
            Err(ClientOrderIdError::InvalidPrefix)
        );
    }
}
