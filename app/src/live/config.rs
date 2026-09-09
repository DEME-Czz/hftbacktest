use std::path::PathBuf;

use super::risk::RiskConfig;
use hftbacktest::strategy::{BuiltinStrategy, BuiltinStrategyConfig, GridConfig};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub strategies: Vec<LiveStrategyConfig>,
    #[serde(default)]
    pub risk: RiskConfig,
    #[serde(default)]
    pub safety: SafetyConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SafetyConfig {
    #[serde(default = "default_stale_market_timeout_ms")]
    pub stale_market_timeout_ms: u64,
    #[serde(default)]
    pub kill_switch_file: Option<PathBuf>,
    #[serde(default = "default_shutdown_cancel_timeout_ms")]
    pub shutdown_cancel_timeout_ms: u64,
}

fn default_stale_market_timeout_ms() -> u64 {
    5_000
}

fn default_shutdown_cancel_timeout_ms() -> u64 {
    10_000
}

fn default_inventory_reduce_threshold() -> f64 {
    0.60
}

fn default_inventory_stop_threshold() -> f64 {
    0.80
}

fn default_requote_ticks() -> u64 {
    5
}

fn default_min_quote_lifetime_ms() -> u64 {
    500
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            stale_market_timeout_ms: default_stale_market_timeout_ms(),
            kill_switch_file: None,
            shutdown_cancel_timeout_ms: default_shutdown_cancel_timeout_ms(),
        }
    }
}

impl SafetyConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.stale_market_timeout_ms == 0
            || self.shutdown_cancel_timeout_ms == 0
            || self
                .kill_switch_file
                .as_ref()
                .is_some_and(|path| path.as_os_str().is_empty())
        {
            return Err("invalid live safety configuration");
        }
        Ok(())
    }
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
        #[serde(default = "default_inventory_reduce_threshold")]
        inventory_reduce_threshold: f64,
        #[serde(default = "default_inventory_stop_threshold")]
        inventory_stop_threshold: f64,
        #[serde(default = "default_requote_ticks")]
        requote_ticks: u64,
        #[serde(default = "default_min_quote_lifetime_ms")]
        min_quote_lifetime_ms: u64,
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
                inventory_reduce_threshold,
                inventory_stop_threshold,
                requote_ticks,
                min_quote_lifetime_ms,
            } => BuiltinStrategyConfig::Grid(GridConfig {
                relative_half_spread: *relative_half_spread,
                relative_grid_interval: *relative_grid_interval,
                grid_num: *grid_num,
                min_grid_step: *min_grid_step,
                skew: *skew,
                order_qty: *order_qty,
                max_position: *max_position,
                inventory_reduce_threshold: *inventory_reduce_threshold,
                inventory_stop_threshold: *inventory_stop_threshold,
                requote_ticks: *requote_ticks,
                min_quote_lifetime_ms: *min_quote_lifetime_ms,
            }),
        };
        BuiltinStrategy::from_config(config)
    }
}
