use std::{
    fmt::Debug,
    time::{Duration, Instant},
};

use hftbacktest::{
    depth::MarketDepth,
    prelude::{Bot, ElapseResult, INVALID_MAX, INVALID_MIN, StateValues},
};

pub fn wait_for_account_state<MD, I>(
    hbt: &mut I,
    asset_no: usize,
    timeout: Duration,
) -> Result<StateValues, String>
where
    MD: MarketDepth,
    I: Bot<MD>,
    I::Error: Debug,
{
    let started_at = Instant::now();
    loop {
        let state = hbt.state_values(asset_no);
        let depth = hbt.depth(asset_no);
        if account_is_ready(state)
            && depth.best_bid_tick() != INVALID_MIN
            && depth.best_ask_tick() != INVALID_MAX
        {
            return Ok(state.clone());
        }
        if started_at.elapsed() >= timeout {
            return Err(format!(
                "live state was not initialized within {timeout:?}; account_ready={}, \
                 best_bid_tick={}, best_ask_tick={}, last state: {state:?}",
                account_is_ready(state),
                depth.best_bid_tick(),
                depth.best_ask_tick(),
            ));
        }
        if hbt
            .elapse(100_000_000)
            .map_err(|error| format!("{error:?}"))?
            == ElapseResult::EndOfData
        {
            return Err("connector stopped before account state was initialized".to_string());
        }
    }
}

fn account_is_ready(state: &StateValues) -> bool {
    state.balance.is_finite()
}

#[cfg(test)]
mod tests {
    use hftbacktest::prelude::StateValues;

    use super::account_is_ready;

    #[test]
    fn requires_a_received_finite_wallet_balance() {
        assert!(!account_is_ready(&StateValues {
            balance: f64::NAN,
            ..StateValues::default()
        }));
        assert!(account_is_ready(&StateValues::default()));
        assert!(account_is_ready(&StateValues {
            balance: 100.0,
            ..StateValues::default()
        }));
    }
}
