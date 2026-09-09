use std::collections::HashMap;

use hftbacktest::{
    depth::{HashMapMarketDepth, L2MarketDepth},
    strategy::{GridConfig, GridStrategy, MarketContext, Strategy, StrategyCommand},
    types::{OrdType, Order, Side, Status, TimeInForce},
};

fn config() -> GridConfig {
    GridConfig {
        relative_half_spread: 0.0005,
        relative_grid_interval: 0.0005,
        grid_num: 3,
        min_grid_step: 0.1,
        skew: 0.0005,
        order_qty: 0.01,
        max_position: 0.03,
        inventory_reduce_threshold: 0.60,
        inventory_stop_threshold: 0.80,
        requote_ticks: 5,
        min_quote_lifetime_ms: 500,
    }
}

fn depth() -> HashMapMarketDepth {
    let mut depth = HashMapMarketDepth::new(0.1, 0.001);
    depth.update_bid_depth(100.0, 1.0, 1);
    depth.update_ask_depth(100.1, 1.0, 1);
    depth
}

fn commands_for(
    strategy: &mut GridStrategy,
    depth: &HashMapMarketDepth,
    position: f64,
    orders: &HashMap<u64, Order>,
    timestamp: i64,
) -> Vec<StrategyCommand> {
    let trades = Vec::new();
    let context = MarketContext {
        timestamp,
        depth,
        position,
        orders,
        last_trades: &trades,
    };
    strategy.on_event(&context)
}

#[test]
fn grid_strategy_quotes_both_sides_with_complete_book() {
    let depth = depth();
    let orders = HashMap::new();
    let mut strategy = GridStrategy::new(config()).unwrap();
    let commands = commands_for(&mut strategy, &depth, 0.0, &orders, 1);

    let bids = commands
        .iter()
        .filter(|command| {
            matches!(
                command,
                StrategyCommand::Submit {
                    side: Side::Buy,
                    ..
                }
            )
        })
        .count();
    let asks = commands
        .iter()
        .filter(|command| {
            matches!(
                command,
                StrategyCommand::Submit {
                    side: Side::Sell,
                    ..
                }
            )
        })
        .count();

    assert_eq!(bids, 3);
    assert_eq!(asks, 3);
}

#[test]
fn grid_strategy_waits_for_complete_book() {
    let depth = HashMapMarketDepth::new(0.1, 0.001);
    let orders = HashMap::new();
    let mut strategy = GridStrategy::new(config()).unwrap();

    assert!(commands_for(&mut strategy, &depth, 0.0, &orders, 1).is_empty());
}

#[test]
fn grid_config_rejects_invalid_order_quantity() {
    let mut invalid = config();
    invalid.order_qty = 0.0;
    assert!(GridStrategy::new(invalid).is_err());
}

#[test]
fn grid_config_rejects_invalid_inventory_thresholds() {
    let mut invalid = config();
    invalid.inventory_reduce_threshold = 0.9;
    invalid.inventory_stop_threshold = 0.8;
    assert!(GridStrategy::new(invalid).is_err());
}

#[test]
fn grid_config_rejects_non_finite_parameters() {
    let mut invalid = config();
    invalid.relative_grid_interval = f64::NAN;
    assert!(GridStrategy::new(invalid).is_err());

    let mut invalid = config();
    invalid.skew = f64::INFINITY;
    assert!(GridStrategy::new(invalid).is_err());
}

#[test]
fn inventory_skew_is_normalized_by_max_position() {
    let depth = depth();
    let orders = HashMap::new();
    let mut cfg = config();
    cfg.skew = 0.01;
    cfg.max_position = 1.0;
    cfg.order_qty = 0.01;
    let mut strategy = GridStrategy::new(cfg).unwrap();

    let flat = commands_for(&mut strategy, &depth, 0.0, &orders, 1);
    let half_long = commands_for(&mut strategy, &depth, 0.5, &orders, 1);

    let best_flat_bid = flat
        .iter()
        .filter_map(|command| match command {
            StrategyCommand::Submit {
                price,
                side: Side::Buy,
                ..
            } => Some(*price),
            _ => None,
        })
        .fold(f64::NEG_INFINITY, f64::max);
    let best_long_bid = half_long
        .iter()
        .filter_map(|command| match command {
            StrategyCommand::Submit {
                price,
                side: Side::Buy,
                ..
            } => Some(*price),
            _ => None,
        })
        .fold(f64::NEG_INFINITY, f64::max);

    assert!(best_long_bid < best_flat_bid);
    assert!(
        best_long_bid > 99.0,
        "50% inventory must not create an order-size-scaled skew"
    );
}

#[test]
fn defensive_inventory_reduces_only_risk_increasing_quote_size() {
    let depth = depth();
    let orders = HashMap::new();
    let mut cfg = config();
    cfg.max_position = 1.0;
    cfg.order_qty = 0.01;
    let mut strategy = GridStrategy::new(cfg).unwrap();

    let commands = commands_for(&mut strategy, &depth, 0.70, &orders, 1);
    let bid_qty = commands.iter().find_map(|command| match command {
        StrategyCommand::Submit {
            qty,
            side: Side::Buy,
            ..
        } => Some(*qty),
        _ => None,
    });
    let ask_qty = commands.iter().find_map(|command| match command {
        StrategyCommand::Submit {
            qty,
            side: Side::Sell,
            ..
        } => Some(*qty),
        _ => None,
    });

    assert_eq!(bid_qty, Some(0.005));
    assert_eq!(ask_qty, Some(0.01));
}

#[test]
fn stop_inventory_zone_disables_risk_increasing_side() {
    let depth = depth();
    let orders = HashMap::new();
    let mut cfg = config();
    cfg.max_position = 1.0;
    let mut strategy = GridStrategy::new(cfg).unwrap();

    let commands = commands_for(&mut strategy, &depth, 0.85, &orders, 1);
    assert!(!commands.iter().any(|command| matches!(
        command,
        StrategyCommand::Submit {
            side: Side::Buy,
            ..
        }
    )));
    assert!(commands.iter().any(|command| matches!(
        command,
        StrategyCommand::Submit {
            side: Side::Sell,
            ..
        }
    )));
}

#[test]
fn stop_zone_reducing_quotes_cannot_cross_through_flat() {
    let depth = depth();
    let orders = HashMap::new();
    let mut cfg = config();
    cfg.max_position = 1.0;
    cfg.order_qty = 0.5;
    let mut strategy = GridStrategy::new(cfg).unwrap();

    let long_commands = commands_for(&mut strategy, &depth, 0.85, &orders, 1);
    let sell_qty: f64 = long_commands
        .iter()
        .filter_map(|command| match command {
            StrategyCommand::Submit {
                qty,
                side: Side::Sell,
                ..
            } => Some(*qty),
            _ => None,
        })
        .sum();
    assert!(sell_qty <= 0.85 + f64::EPSILON);
    assert!(!long_commands.iter().any(|command| matches!(
        command,
        StrategyCommand::Submit {
            side: Side::Buy,
            ..
        }
    )));

    let short_commands = commands_for(&mut strategy, &depth, -0.85, &orders, 1);
    let buy_qty: f64 = short_commands
        .iter()
        .filter_map(|command| match command {
            StrategyCommand::Submit {
                qty,
                side: Side::Buy,
                ..
            } => Some(*qty),
            _ => None,
        })
        .sum();
    assert!(buy_qty <= 0.85 + f64::EPSILON);
    assert!(!short_commands.iter().any(|command| matches!(
        command,
        StrategyCommand::Submit {
            side: Side::Sell,
            ..
        }
    )));
}

#[test]
fn stop_zone_cancels_risk_increasing_quote_without_waiting_for_min_lifetime() {
    let depth = depth();
    let mut cfg = config();
    cfg.max_position = 1.0;
    let mut strategy = GridStrategy::new(cfg).unwrap();
    let mut order = Order::new(
        900,
        990,
        0.1,
        0.01,
        Side::Buy,
        OrdType::Limit,
        TimeInForce::GTX,
    );
    order.status = Status::New;
    order.local_timestamp = 1_000_000_000;
    let orders = HashMap::from([(order.order_id, order)]);

    let commands = commands_for(&mut strategy, &depth, 0.85, &orders, 1_100_000_000);
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, StrategyCommand::Cancel { order_id: 900 }))
    );
}

#[test]
fn requote_hysteresis_keeps_a_nearby_existing_quote() {
    let depth = depth();
    let mut strategy = GridStrategy::new(config()).unwrap();
    let empty = HashMap::new();
    let initial = commands_for(&mut strategy, &depth, 0.0, &empty, 1_000_000_000);
    let (order_id, price) = initial
        .iter()
        .find_map(|command| match command {
            StrategyCommand::Submit {
                order_id,
                price,
                side: Side::Buy,
                ..
            } => Some((*order_id, *price)),
            _ => None,
        })
        .unwrap();

    let mut order = Order::new(
        order_id,
        (price / 0.1).round() as i64 + 2,
        0.1,
        0.01,
        Side::Buy,
        OrdType::Limit,
        TimeInForce::GTX,
    );
    order.status = Status::New;
    order.local_timestamp = 1;
    let orders = HashMap::from([(order_id, order)]);

    let commands = commands_for(&mut strategy, &depth, 0.0, &orders, 2_000_000_000);
    assert!(!commands.iter().any(|command| matches!(
        command,
        StrategyCommand::Cancel { order_id: canceled } if *canceled == order_id
    )));
}

#[test]
fn minimum_quote_lifetime_blocks_early_cancel() {
    let depth = depth();
    let mut strategy = GridStrategy::new(config()).unwrap();
    let mut order = Order::new(
        900,
        900,
        0.1,
        0.01,
        Side::Buy,
        OrdType::Limit,
        TimeInForce::GTX,
    );
    order.status = Status::New;
    order.local_timestamp = 1_000_000_000;
    let orders = HashMap::from([(order.order_id, order)]);

    let commands = commands_for(&mut strategy, &depth, 0.0, &orders, 1_100_000_000);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, StrategyCommand::Cancel { order_id: 900 }))
    );
}
