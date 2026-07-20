use std::collections::HashSet;

use hftbacktest::types::LiveEvent;

use super::msg::stream::OrderTradeUpdate;

pub fn balance_events<'a>(
    symbols: &HashSet<String>,
    balances: impl Iterator<Item = (&'a str, f64)>,
    exch_ts: i64,
) -> Vec<LiveEvent> {
    let mut balances: Vec<_> = balances.collect();
    balances.sort_unstable_by_key(|(asset, _)| std::cmp::Reverse(asset.len()));

    symbols
        .iter()
        .filter_map(|symbol| {
            balances
                .iter()
                .find(|(asset, _)| symbol.ends_with(asset))
                .map(|(_, balance)| LiveEvent::Balance {
                    symbol: symbol.clone(),
                    balance: *balance,
                    exch_ts,
                })
        })
        .collect()
}

pub fn fill_event(update: &OrderTradeUpdate) -> Option<LiveEvent> {
    let order = &update.order;
    if order.execution_type != "TRADE" || order.order_last_filled_qty <= 0.0 || order.trade_id < 0 {
        return None;
    }

    Some(LiveEvent::Fill {
        symbol: order.symbol.clone(),
        trade_id: order.trade_id,
        qty: order.order_last_filled_qty,
        price: order.last_filled_price,
        fee: order.commission.unwrap_or(0.0),
        exch_ts: order.order_trade_time * 1_000_000,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use hftbacktest::types::LiveEvent;

    use super::{balance_events, fill_event};
    use crate::binancefutures::msg::stream::{EventStream, Stream};

    #[test]
    fn maps_quote_asset_balance_to_registered_symbols() {
        let symbols = HashSet::from(["dogeusdt".to_string(), "btcusdc".to_string()]);
        let events = balance_events(&symbols, [("usdt", 123.0), ("usdc", 45.0)].into_iter(), 99);

        assert!(events.iter().any(|event| matches!(
            event,
            LiveEvent::Balance { symbol, balance, exch_ts }
                if symbol == "dogeusdt" && *balance == 123.0 && *exch_ts == 99
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            LiveEvent::Balance { symbol, balance, .. }
                if symbol == "btcusdc" && *balance == 45.0
        )));
    }

    #[test]
    fn converts_trade_update_to_fill_with_commission() {
        let json = r#"{
            "e":"ORDER_TRADE_UPDATE","E":1568879465651,"T":1568879465650,
            "o":{"s":"DOGEUSDT","c":"bot-1","S":"BUY","o":"LIMIT","f":"GTC",
            "q":"10","p":"0.07260","ap":"0.07263","sp":"0","x":"TRADE","X":"PARTIALLY_FILLED",
            "i":8886774,"l":"10","z":"10","L":"0.07263","N":"USDT","n":"0.0003",
            "T":1568879465650,"t":12345}
        }"#;
        let Stream::EventStream(EventStream::OrderTradeUpdate(update)) =
            serde_json::from_str(json).unwrap()
        else {
            panic!("expected an order trade update");
        };

        assert!(matches!(
            fill_event(&update),
            Some(LiveEvent::Fill { trade_id: 12345, qty, price, fee, .. })
                if qty == 10.0 && price == 0.07263 && fee == 0.0003
        ));
    }

    #[test]
    fn ignores_non_trade_order_updates() {
        let json = r#"{
            "e":"ORDER_TRADE_UPDATE","E":1568879465651,"T":1568879465650,
            "o":{"s":"DOGEUSDT","c":"bot-1","S":"BUY","o":"LIMIT","f":"GTC",
            "q":"10","p":"0.07260","ap":"0","sp":"0","x":"NEW","X":"NEW",
            "i":8886774,"l":"0","z":"0","L":"0","N":null,"n":"0",
            "T":1568879465650,"t":0}
        }"#;
        let Stream::EventStream(EventStream::OrderTradeUpdate(update)) =
            serde_json::from_str(json).unwrap()
        else {
            panic!("expected an order trade update");
        };

        assert!(fill_event(&update).is_none());
    }
}
