//! L2 order-book-driven high-frequency trading engine.
//!
//! The engine is intentionally pure computation: deterministic market replay, order books,
//! queue-position estimation, latency, partial-fill simulation, fees, positions, and PnL.
//! Exchange networking and asynchronous runtimes belong to the `app` crate.

#[cfg(any(feature = "backtest", doc))]
pub mod backtest;
pub mod depth;
pub mod live;
pub mod prelude;
pub mod types;
mod utils;
