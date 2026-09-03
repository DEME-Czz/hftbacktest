use hftbacktest::{
    backtest::models::{QueueModel, RiskAdverseQueueModel},
    depth::{HashMapMarketDepth, L2MarketDepth, MarketDepth},
    types::{OrdType, Order, Side, TimeInForce},
};

#[test]
fn l2_queue_fill_golden() {
    let mut depth = HashMapMarketDepth::new(1.0, 1.0);
    depth.update_bid_depth(100.0, 10.0, 1);
    depth.update_ask_depth(101.0, 8.0, 1);

    assert_eq!(depth.best_bid(), 100.0);
    assert_eq!(depth.best_ask(), 101.0);

    let model = RiskAdverseQueueModel::<HashMapMarketDepth>::new();
    let mut order = Order::new(
        1,
        100,
        1.0,
        1.0,
        Side::Buy,
        OrdType::Limit,
        TimeInForce::GTC,
    );
    model.new_order(&mut order, &depth);
    model.trade(&mut order, 11.0, &depth);

    assert_eq!(model.is_filled(&mut order, &depth), 1.0);
}
