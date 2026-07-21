use std::collections::{HashSet, VecDeque};

pub(super) struct SubscriptionTracker {
    pending: VecDeque<String>,
    subscribed: HashSet<String>,
}

impl SubscriptionTracker {
    pub(super) fn new(initial_symbols: impl IntoIterator<Item = String>) -> Self {
        let mut pending = VecDeque::new();
        let mut subscribed = HashSet::new();
        for symbol in initial_symbols {
            if subscribed.insert(symbol.clone()) {
                pending.push_back(symbol);
            }
        }
        Self {
            pending,
            subscribed,
        }
    }

    pub(super) fn next_initial(&mut self) -> Option<String> {
        self.pending.pop_front()
    }

    pub(super) fn accept(&mut self, symbol: String) -> bool {
        self.subscribed.insert(symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::SubscriptionTracker;

    #[test]
    fn replays_registered_symbols_on_new_connection() {
        let mut tracker = SubscriptionTracker::new(["dogeusdt".to_string(), "btcusdt".to_string()]);

        let mut symbols = Vec::new();
        while let Some(symbol) = tracker.next_initial() {
            symbols.push(symbol);
        }
        symbols.sort();

        assert_eq!(symbols, ["btcusdt", "dogeusdt"]);
    }

    #[test]
    fn ignores_broadcast_duplicate_of_replayed_symbol() {
        let mut tracker = SubscriptionTracker::new(["dogeusdt".to_string()]);

        assert_eq!(tracker.next_initial().as_deref(), Some("dogeusdt"));
        assert!(!tracker.accept("dogeusdt".to_string()));
    }

    #[test]
    fn accepts_symbol_registered_after_connection() {
        let mut tracker = SubscriptionTracker::new(["dogeusdt".to_string()]);
        assert_eq!(tracker.next_initial().as_deref(), Some("dogeusdt"));

        assert!(tracker.accept("ethusdt".to_string()));
        assert!(!tracker.accept("ethusdt".to_string()));
    }
}
