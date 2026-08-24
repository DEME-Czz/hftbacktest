#[cfg(test)]
mod tests {
    use super::SafetyState;

    #[test]
    fn stale_market_at_deadline_blocks_until_orders_are_gone_and_market_is_fresh() {
        let mut safety = SafetyState::new(1_000, ["btcusdt"]);
        safety.on_market_batch("btcusdt", 0, false);

        safety.on_tick(999);
        assert!(safety.can_submit("btcusdt", 999));
        safety.on_tick(1_000);
        assert!(safety.is_halted("btcusdt"));
        assert!(!safety.can_submit("btcusdt", 1_000));

        safety.on_market_batch("btcusdt", 1_001, true);
        assert!(safety.is_halted("btcusdt"));
        safety.on_market_batch("btcusdt", 1_002, false);
        assert!(safety.can_submit("btcusdt", 1_002));
    }

    #[test]
    fn kill_switch_latches_every_symbol_until_process_restart() {
        let mut safety = SafetyState::new(1_000, ["btcusdt", "ethusdt"]);
        safety.on_market_batch("btcusdt", 0, false);
        safety.on_market_batch("ethusdt", 0, false);

        assert!(safety.trip_kill_switch());
        assert!(!safety.trip_kill_switch());
        for symbol in ["btcusdt", "ethusdt"] {
            safety.on_market_batch(symbol, 1, false);
            assert!(safety.is_halted(symbol));
            assert!(!safety.can_submit(symbol, 1));
        }
    }
}
