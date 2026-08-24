use hftbacktest::{strategy::StrategyCommand, types::Side};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct RiskConfig {
    #[serde(default = "default_max_order_qty")]
    pub max_order_qty: f64,
    #[serde(default = "default_max_order_notional")]
    pub max_order_notional: f64,
    #[serde(default = "default_max_position")]
    pub max_position: f64,
    #[serde(default = "default_max_open_orders")]
    pub max_open_orders: usize,
}

fn default_max_order_qty() -> f64 { 0.001 }
fn default_max_order_notional() -> f64 { 100.0 }
fn default_max_position() -> f64 { 0.003 }
fn default_max_open_orders() -> usize { 6 }

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_order_qty: default_max_order_qty(),
            max_order_notional: default_max_order_notional(),
            max_position: default_max_position(),
            max_open_orders: default_max_open_orders(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RiskGate {
    config: RiskConfig,
}

impl RiskGate {
    pub fn new(config: RiskConfig) -> Self { Self { config } }

    pub fn allow(
        &self,
        command: &StrategyCommand,
        position: f64,
        open_orders: usize,
        same_side_order_exposure: f64,
    ) -> Result<(), &'static str> {
        match command {
            StrategyCommand::Submit { price, qty, side, .. } => {
                if !price.is_finite() || *price <= 0.0 || !qty.is_finite() || *qty <= 0.0 {
                    return Err("invalid order price or quantity");
                }
                if *qty > self.config.max_order_qty {
                    return Err("max_order_qty exceeded");
                }
                if price * qty > self.config.max_order_notional {
                    return Err("max_order_notional exceeded");
                }
                if open_orders >= self.config.max_open_orders {
                    return Err("max_open_orders exceeded");
                }
                if !same_side_order_exposure.is_finite() || same_side_order_exposure < 0.0 {
                    return Err("invalid active order exposure");
                }
                let projected = match side {
                    Side::Buy => position + same_side_order_exposure + qty,
                    Side::Sell => position - same_side_order_exposure - qty,
                    _ => return Err("unsupported order side"),
                };
                if projected.abs() > self.config.max_position {
                    return Err("max_position exceeded");
                }
                Ok(())
            }
            StrategyCommand::Modify { price, qty, .. } => {
                if !price.is_finite() || *price <= 0.0 || !qty.is_finite() || *qty <= 0.0 {
                    return Err("invalid modify price or quantity");
                }
                if *qty > self.config.max_order_qty || price * qty > self.config.max_order_notional {
                    return Err("modify risk limit exceeded");
                }
                Ok(())
            }
            StrategyCommand::Cancel { .. } => Ok(()),
        }
    }
}
