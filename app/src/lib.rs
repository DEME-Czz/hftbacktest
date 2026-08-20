pub mod binancefutures;
pub mod connector;
pub mod utils;

// Temporary parsing compatibility shim. It is not an exchange adapter and will be removed when
// legacy multi-exchange helpers are fully split into Binance-specific utilities.
mod bybit;
