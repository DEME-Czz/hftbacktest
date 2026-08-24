use hftbacktest::strategy::{BuiltinStrategy, BuiltinStrategyConfig, GridConfig};
use serde::Deserialize;
use super::risk::RiskConfig;

#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub strategies: Vec<LiveStrategyConfig>,
    #[serde(default)]
    pub risk: RiskConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LiveStrategyConfig {
    pub symbol: String,
    pub tick_size: f64,
    pub lot_size: f64,
    #[serde(flatten)]
    pub strategy: StrategyConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrategyConfig {
    Grid {
        relative_half_spread: f64,
        relative_grid_interval: f64,
        grid_num: usize,
        min_grid_step: f64,
        skew: f64,
        order_qty: f64,
        max_position: f64,
    },
}

impl LiveStrategyConfig {
    pub fn build_strategy(&self) -> Result<BuiltinStrategy, &'static str> {
        let config = match &self.strategy {
            StrategyConfig::Grid {
                relative_half_spread,
                relative_grid_interval,
                grid_num,
                min_grid_step,
                skew,
                order_qty,
                max_position,
            } => BuiltinStrategyConfig::Grid(GridConfig {
                relative_half_spread: *relative_half_spread,
                relative_grid_interval: *relative_grid_interval,
                grid_num: *grid_num,
                min_grid_step: *min_grid_step,
                skew: *skew,
                order_qty: *order_qty,
                max_position: *max_position,
            }),
        };
        BuiltinStrategy::from_config(config)
    }
}
