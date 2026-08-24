use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default)]
struct SymbolSafety {
    last_market_ms: Option<u64>,
    stale: bool,
}

/// Deterministic safety state driven by a monotonic clock supplied by the live service.
pub struct SafetyState {
    stale_timeout_ms: u64,
    symbols: HashMap<String, SymbolSafety>,
    kill_latched: bool,
}

impl SafetyState {
    pub fn new<I, S>(stale_timeout_ms: u64, symbols: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            stale_timeout_ms,
            symbols: symbols
                .into_iter()
                .map(|symbol| (symbol.into(), SymbolSafety::default()))
                .collect(),
            kill_latched: false,
        }
    }

    pub fn on_market_batch(&mut self, symbol: &str, now_ms: u64, has_open_orders: bool) {
        let Some(state) = self.symbols.get_mut(symbol) else {
            return;
        };
        let crossed_deadline = state.last_market_ms.map_or(now_ms >= self.stale_timeout_ms, |last| {
            now_ms.saturating_sub(last) >= self.stale_timeout_ms
        });
        state.last_market_ms = Some(now_ms);
        if crossed_deadline {
            state.stale = true;
        } else if !has_open_orders {
            state.stale = false;
        }
    }

    pub fn on_tick(&mut self, now_ms: u64) {
        for state in self.symbols.values_mut() {
            let age_ms = state
                .last_market_ms
                .map_or(now_ms, |last| now_ms.saturating_sub(last));
            if age_ms >= self.stale_timeout_ms {
                state.stale = true;
            }
        }
    }

    pub fn mark_disconnected(&mut self) {
        for state in self.symbols.values_mut() {
            state.stale = true;
        }
    }

    pub fn halt_symbol(&mut self, symbol: &str) {
        if let Some(state) = self.symbols.get_mut(symbol) {
            state.stale = true;
        }
    }

    pub fn trip_kill_switch(&mut self) -> bool {
        !std::mem::replace(&mut self.kill_latched, true)
    }

    pub fn is_halted(&self, symbol: &str) -> bool {
        self.kill_latched
            || self
                .symbols
                .get(symbol)
                .is_none_or(|state| state.stale || state.last_market_ms.is_none())
    }

    pub fn requires_cancel(&self, symbol: &str) -> bool {
        self.kill_latched || self.symbols.get(symbol).is_some_and(|state| state.stale)
    }

    pub fn can_submit(&self, symbol: &str, now_ms: u64) -> bool {
        if self.kill_latched {
            return false;
        }
        self.symbols.get(symbol).is_some_and(|state| {
            !state.stale
                && state
                    .last_market_ms
                    .is_some_and(|last| now_ms.saturating_sub(last) < self.stale_timeout_ms)
        })
    }

    pub fn kill_latched(&self) -> bool {
        self.kill_latched
    }
}

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
    fn market_batch_cannot_hide_a_gap_that_already_crossed_the_deadline() {
        let mut safety = SafetyState::new(1_000, ["btcusdt"]);
        safety.on_market_batch("btcusdt", 0, false);

        // The timer has not fired yet, but the old market is already beyond its deadline.
        safety.on_market_batch("btcusdt", 1_000, false);
        assert!(safety.is_halted("btcusdt"));
        assert!(!safety.can_submit("btcusdt", 1_000));

        // Recovery requires another genuinely fresh batch after the stale transition.
        safety.on_market_batch("btcusdt", 1_001, false);
        assert!(safety.can_submit("btcusdt", 1_001));
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
