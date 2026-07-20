use hftbacktest::types::Side;
use std::{
    sync::Mutex as StdMutex,
    time::{Duration, Instant},
};

use tokio::sync::{Mutex, MutexGuard};

use super::{BinanceFuturesError, msg::rest::AccountInformationV3, rest::BinanceFuturesClient};

pub(super) struct MarginGuard {
    gate: Mutex<()>,
    rejection: StdMutex<Option<(Instant, MarginDecision)>>,
}

impl Default for MarginGuard {
    fn default() -> Self {
        Self {
            gate: Mutex::new(()),
            rejection: StdMutex::new(None),
        }
    }
}

impl MarginGuard {
    pub(super) async fn lock(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().await
    }

    pub(super) async fn check(
        &self,
        client: &BinanceFuturesClient,
        symbol: &str,
        side: Side,
        price: f64,
        quantity: f64,
    ) -> Result<MarginDecision, BinanceFuturesError> {
        if let Some(decision) = self.cached_rejection() {
            return Ok(decision);
        }

        let account = client.get_account_information().await?;
        let decision = margin_decision(&account, symbol, side, price, quantity)
            .ok_or(BinanceFuturesError::InstrumentNotFound)?;
        if decision.is_sufficient() {
            *self.rejection.lock().unwrap() = None;
        } else {
            *self.rejection.lock().unwrap() = Some((Instant::now(), decision));
        }
        Ok(decision)
    }

    fn cached_rejection(&self) -> Option<MarginDecision> {
        const REJECTION_COOLDOWN: Duration = Duration::from_secs(5);

        self.rejection
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(created_at, _)| created_at.elapsed() < REJECTION_COOLDOWN)
            .map(|(_, decision)| *decision)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MarginDecision {
    pub required_balance: f64,
    pub available_balance: f64,
}

impl MarginDecision {
    pub(super) fn is_sufficient(self) -> bool {
        self.required_balance.is_finite()
            && self.available_balance.is_finite()
            && self.required_balance >= 0.0
            && self.available_balance >= self.required_balance
    }
}

pub(super) fn required_balance(side: Side, price: f64, quantity: f64) -> f64 {
    if !price.is_finite() || !quantity.is_finite() || price <= 0.0 || quantity <= 0.0 {
        return f64::INFINITY;
    }

    match side {
        Side::Buy | Side::Sell => {}
        Side::None | Side::Unsupported => return f64::INFINITY,
    }
    quantity * price
}

fn margin_decision(
    account: &AccountInformationV3,
    symbol: &str,
    side: Side,
    price: f64,
    quantity: f64,
) -> Option<MarginDecision> {
    let asset = account
        .assets
        .iter()
        .filter(|asset| symbol.ends_with(&asset.asset))
        .max_by_key(|asset| asset.asset.len())?;
    Some(MarginDecision {
        required_balance: required_balance(side, price, quantity),
        available_balance: asset.available_balance,
    })
}

#[cfg(test)]
mod tests {
    use super::{MarginDecision, margin_decision, required_balance};
    use crate::binancefutures::msg::rest::AccountInformationV3;
    use hftbacktest::types::Side;

    #[test]
    fn requires_full_notional_for_a_new_position() {
        assert_eq!(required_balance(Side::Buy, 2.5, 10.0), 25.0);
        assert_eq!(required_balance(Side::Sell, 2.5, 10.0), 25.0);
    }

    #[test]
    fn rejects_missing_or_unsupported_order_sides() {
        assert!(required_balance(Side::None, 2.5, 10.0).is_infinite());
        assert!(required_balance(Side::Unsupported, 2.5, 10.0).is_infinite());
    }

    #[test]
    fn rejects_non_finite_or_insufficient_balance() {
        assert!(
            !MarginDecision {
                required_balance: 10.0,
                available_balance: f64::NAN,
            }
            .is_sufficient()
        );
        assert!(
            !MarginDecision {
                required_balance: 10.0,
                available_balance: 9.99,
            }
            .is_sufficient()
        );
        assert!(
            MarginDecision {
                required_balance: 10.0,
                available_balance: 10.0,
            }
            .is_sufficient()
        );
    }

    #[test]
    fn uses_the_longest_matching_quote_asset() {
        let account: AccountInformationV3 = serde_json::from_str(
            r#"{
                "assets":[
                    {"asset":"USD","walletBalance":"100","availableBalance":"90","updateTime":1},
                    {"asset":"USDT","walletBalance":"20","availableBalance":"12","updateTime":1}
                ],
                "positions":[
                    {"symbol":"SYNUSDT","positionAmt":"-8","updateTime":1}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            margin_decision(&account, "synusdt", Side::Buy, 2.5, 10.0),
            Some(MarginDecision {
                required_balance: 25.0,
                available_balance: 12.0,
            })
        );
    }
}
