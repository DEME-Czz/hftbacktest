//! L2 order-book-driven high-frequency trading engine.
//!
//! Pure computation only: deterministic market replay, L2 order books, queue-position estimation,
//! latency, partial-fill simulation, fees, positions, PnL, and an exchange-independent strategy API.

#[cfg(any(feature = "backtest", doc))]
pub mod backtest;
pub mod depth;
pub mod live;
pub mod prelude;
pub mod strategy;
pub mod types;
mod utils;
