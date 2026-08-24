use hashbrown::Equivalent;
use hftbacktest::prelude::OrderId;
use rand::Rng;

#[derive(Eq, Hash, PartialEq, Debug)]
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
