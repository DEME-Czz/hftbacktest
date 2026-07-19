use std::time::{Duration, Instant};

use hftbacktest::types::{
    Event, LOCAL_ASK_DEPTH_EVENT, LOCAL_BID_DEPTH_EVENT, LOCAL_BUY_TRADE_EVENT, LiveEvent,
};
use hftbacktest_tui::{AppState, Health};

fn event(ev: u64, px: f64, qty: f64) -> Event {
    Event {
        ev,
        exch_ts: 1_000,
        local_ts: 1_250,
        px,
        qty,
        order_id: 0,
        ival: 0,
        fval: 0.0,
    }
}

#[test]
fn feed_events_build_depth_and_trades_for_selected_symbol() {
    let mut app = AppState::new("dogeusdt", 0.00001, 1.0, 100);
    app.apply(LiveEvent::Feed {
        symbol: "dogeusdt".into(),
        event: event(LOCAL_BID_DEPTH_EVENT, 0.18342, 500.0),
    });
    app.apply(LiveEvent::Feed {
        symbol: "dogeusdt".into(),
        event: event(LOCAL_ASK_DEPTH_EVENT, 0.18343, 600.0),
    });
    app.apply(LiveEvent::Feed {
        symbol: "dogeusdt".into(),
        event: event(LOCAL_BUY_TRADE_EVENT, 0.18343, 20.0),
    });
    app.apply(LiveEvent::Feed {
        symbol: "btcusdt".into(),
        event: event(LOCAL_BID_DEPTH_EVENT, 100_000.0, 1.0),
    });

    let (bid, bid_qty) = app.best_bid().expect("best bid");
    let (ask, ask_qty) = app.best_ask().expect("best ask");
    assert!((bid - 0.18342).abs() < 1e-12);
    assert!((ask - 0.18343).abs() < 1e-12);
    assert_eq!(bid_qty, 500.0);
    assert_eq!(ask_qty, 600.0);
    assert_eq!(app.recent_trades().len(), 1);
    assert_eq!(app.last_feed_latency_ns(), Some(250));
}

#[test]
fn health_uses_feed_age_thresholds() {
    let mut app = AppState::new("dogeusdt", 0.00001, 1.0, 100);
    let now = Instant::now();
    app.note_feed_at(now);

    assert_eq!(app.health_at(now + Duration::from_secs(1)), Health::Active);
    assert_eq!(app.health_at(now + Duration::from_secs(5)), Health::Stale);
    assert_eq!(
        app.health_at(now + Duration::from_secs(11)),
        Health::Disconnected
    );
}

#[test]
fn zero_quantity_removes_depth_level() {
    let mut app = AppState::new("dogeusdt", 0.00001, 1.0, 100);
    app.apply(LiveEvent::Feed {
        symbol: "dogeusdt".into(),
        event: event(LOCAL_BID_DEPTH_EVENT, 0.18342, 500.0),
    });
    app.apply(LiveEvent::Feed {
        symbol: "dogeusdt".into(),
        event: event(LOCAL_BID_DEPTH_EVENT, 0.18342, 0.0),
    });

    assert_eq!(app.best_bid(), None);
}
