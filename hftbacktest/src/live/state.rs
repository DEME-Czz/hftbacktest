use std::collections::HashSet;

use crate::types::StateValues;

pub(super) struct LiveState {
    values: StateValues,
    trade_ids: HashSet<i64>,
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            values: StateValues {
                // Zero is a valid exchange wallet balance. NaN represents that no balance event
                // has been received yet and prevents startup code from confusing the two states.
                balance: f64::NAN,
                ..StateValues::default()
            },
            trade_ids: HashSet::new(),
        }
    }
}

impl LiveState {
    pub fn set_position(&mut self, qty: f64) {
        self.values.position = qty;
    }

    pub fn set_balance(&mut self, balance: f64) {
        self.values.balance = balance;
    }

    pub fn apply_fill(&mut self, trade_id: i64, qty: f64, price: f64, fee: f64) {
        if qty <= 0.0 || !self.trade_ids.insert(trade_id) {
            return;
        }
        self.values.fee += fee;
        self.values.num_trades += 1;
        self.values.trading_volume += qty;
        self.values.trading_value += qty * price;
    }

    pub fn values(&self) -> &StateValues {
        &self.values
    }
}

#[cfg(test)]
mod tests {
    use super::LiveState;

    #[test]
    fn updates_position_and_balance() {
        let mut state = LiveState::default();
        assert!(state.values().balance.is_nan());
        state.set_position(156.0);
        state.set_balance(125.5);

        assert_eq!(state.values().position, 156.0);
        assert_eq!(state.values().balance, 125.5);
    }

    #[test]
    fn accumulates_unique_fills() {
        let mut state = LiveState::default();
        state.apply_fill(7, 10.0, 0.07263, 0.0003);
        state.apply_fill(8, 5.0, 0.07264, 0.0002);
        state.apply_fill(7, 10.0, 0.07263, 0.0003);

        let values = state.values();
        assert_eq!(values.num_trades, 2);
        assert_eq!(values.trading_volume, 15.0);
        assert!((values.trading_value - 1.0895).abs() < 1e-12);
        assert!((values.fee - 0.0005).abs() < 1e-12);
    }

    #[test]
    fn ignores_non_fill_updates() {
        let mut state = LiveState::default();
        state.apply_fill(7, 0.0, 0.07263, 0.0003);
        assert_eq!(state.values().num_trades, 0);
    }
}
