use crate::{
    depth::MarketDepth,
    strategy::{GridConfig, GridStrategy, MarketContext, Strategy, StrategyCommand},
};

/// Configuration for strategies shipped with the engine.
///
/// Add a new variant when a strategy becomes part of the maintained built-in set. Exchange-specific
/// configuration must never be added here.
#[derive(Clone, Debug)]
pub enum BuiltinStrategyConfig {
    Grid(GridConfig),
}

pub enum BuiltinStrategy {
    Grid(GridStrategy),
}

impl BuiltinStrategy {
    pub fn from_config(config: BuiltinStrategyConfig) -> Result<Self, &'static str> {
        match config {
            BuiltinStrategyConfig::Grid(config) => Ok(Self::Grid(GridStrategy::new(config)?)),
        }
    }
}

impl<MD: MarketDepth> Strategy<MD> for BuiltinStrategy {
    fn on_event(&mut self, context: &MarketContext<'_, MD>) -> Vec<StrategyCommand> {
        match self {
            BuiltinStrategy::Grid(strategy) => strategy.on_event(context),
        }
    }
}
