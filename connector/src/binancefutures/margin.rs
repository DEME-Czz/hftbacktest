use hftbacktest::types::Side;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MarginDecision {
    pub required_balance: f64,
    pub available_balance: f64,
}

impl MarginDecision {
    pub(super) fn is_sufficient(self) -> bool {
        todo!("implemented after RED tests")
    }
}

pub(super) fn required_balance(
    side: Side,
    position: f64,
    price: f64,
    quantity: f64,
) -> f64 {
    let _ = (side, position, price, quantity);
    todo!("implemented after RED tests")
}

#[cfg(test)]
mod tests {
    use super::{MarginDecision, required_balance};
    use hftbacktest::types::Side;

    #[test]
    fn requires_full_notional_for_a_new_position() {
        assert_eq!(required_balance(Side::Buy, 0.0, 2.5, 10.0), 25.0);
        assert_eq!(required_balance(Side::Sell, 0.0, 2.5, 10.0), 25.0);
    }

    #[test]
    fn only_reserves_the_position_increasing_quantity() {
        assert_eq!(required_balance(Side::Buy, -8.0, 2.5, 10.0), 5.0);
        assert_eq!(required_balance(Side::Sell, 8.0, 2.5, 10.0), 5.0);
    }

    #[test]
    fn pure_position_reduction_needs_no_new_balance() {
        assert_eq!(required_balance(Side::Buy, -10.0, 2.5, 8.0), 0.0);
        assert_eq!(required_balance(Side::Sell, 10.0, 2.5, 8.0), 0.0);
    }

    #[test]
    fn rejects_non_finite_or_insufficient_balance() {
        assert!(!MarginDecision {
            required_balance: 10.0,
            available_balance: f64::NAN,
        }
        .is_sufficient());
        assert!(!MarginDecision {
            required_balance: 10.0,
            available_balance: 9.99,
        }
        .is_sufficient());
        assert!(MarginDecision {
            required_balance: 10.0,
            available_balance: 10.0,
        }
        .is_sufficient());
    }
}
