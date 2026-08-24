use hashbrown::Equivalent;
use hftbacktest::prelude::OrderId;
use rand::Rng;

#[derive(Eq, Hash, PartialEq, Debug)]
pub struct SymbolOrderId {
    pub symbol: String,
    pub order_id: OrderId,
}

impl SymbolOrderId {
    pub fn new(symbol: String, order_id: OrderId) -> Self { Self { symbol, order_id } }
}

#[derive(Eq, Hash, PartialEq, Debug)]
pub struct RefSymbolOrderId<'a> {
    pub symbol: &'a str,
    pub order_id: OrderId,
}

impl<'a> RefSymbolOrderId<'a> {
    pub fn new(symbol: &'a str, order_id: OrderId) -> Self { Self { symbol, order_id } }
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
