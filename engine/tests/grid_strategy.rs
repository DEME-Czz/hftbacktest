use std::collections::HashMap;

use hftbacktest::{
    depth::{HashMapMarketDepth, L2MarketDepth},
    strategy::{GridConfig, GridStrategy, MarketContext, Strategy, StrategyCommand},
};

fn config() -> GridConfig {
    GridConfig {
        relative_half_spread: 0.0005,
        relative_grid_interval: 0.0005,
        grid_num: 3,
        min_grid_step: 0.1,
        skew: 0.0005 / 3.0,
        order_qty: 0.01,
        max_position: 0.03,
    }
}

#[test]
fn grid_strategy_quotes_both_sides_with_complete_book() {
    let mut depth = HashMapMarketDepth::new(0.1, 0.001);
    depth.update_bid_depth(100.0, 1.0, 1);
    depth.update_ask_depth(100.1, 1.0, 1);

    let orders = HashMap::new();
    let trades = Vec::new();
    let context = MarketContext {
        timestamp: 1,
        depth: &depth,
        position: 0.0,
        orders: &orders,
        last_trades: &trades,
    };

    let mut strategy = GridStrategy::new(config()).unwrap();
    let commands = strategy.on_event(&context);

    let bids = commands
        .iter()
        .filter(|command| matches!(command, StrategyCommand::Submit { side: hftbacktest::types::Side::Buy, .. }))
        .count();
    let asks = commands
        .iter()
        .filter(|command| matches!(command, StrategyCommand::Submit { side: hftbacktest::types::Side::Sell, .. }))
        .count();

    assert_eq!(bids, 3);
    assert_eq!(asks, 3);
}

#[test]
fn grid_strategy_waits_for_complete_book() {
    let depth = HashMapMarketDepth::new(0.1, 0.001);
    let orders = HashMap::new();
    let trades = Vec::new();
    let context = MarketContext {
        timestamp: 1,
        depth: &depth,
        position: 0.0,
        orders: &orders,
        last_trades: &trades,
    };

    let mut strategy = GridStrategy::new(config()).unwrap();
    assert!(strategy.on_event(&context).is_empty());
}

#[test]
fn grid_config_rejects_invalid_order_quantity() {
    let mut invalid = config();
    invalid.order_qty = 0.0;
    assert!(GridStrategy::new(invalid).is_err());
}
