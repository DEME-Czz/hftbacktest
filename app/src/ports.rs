use hftbacktest::types::Order;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunMode {
    DryRun,
    Execute,
}

impl RunMode {
    pub fn from_execute(execute: bool) -> Self {
        if execute {
            Self::Execute
        } else {
            Self::DryRun
        }
    }

    pub fn allows_trading(self) -> bool {
        self == Self::Execute
    }
}

pub enum PublishEvent {
    BatchStart,
    BatchEnd,
    LiveEvent(hftbacktest::types::LiveEvent),
    /// The private account stream is unavailable. Live execution must fail closed until each
    /// configured symbol has been reconciled again.
    AccountStreamDisconnected,
}

/// Public market-data side of an exchange adapter.
pub trait MarketDataSource {
    fn register(&mut self, symbol: String);
    fn start_market_data(&mut self, tx: UnboundedSender<PublishEvent>);
}

/// Authenticated order/account side of an exchange adapter.
pub trait ExecutionVenue {
    fn start_account_stream(&self, tx: UnboundedSender<PublishEvent>);
    fn open_orders(&self, symbol: &str) -> Vec<Order>;
    fn submit(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>);
    fn cancel(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>);
}

pub trait LiveConnector: MarketDataSource + ExecutionVenue {}

impl<T> LiveConnector for T where T: MarketDataSource + ExecutionVenue {}
