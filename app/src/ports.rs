use hftbacktest::types::{Order, Side};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Debug, PartialEq)]
pub struct TradingInstrument {
    pub symbol: String,
    pub tick_size: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunMode {
    DryRun,
    Execute,
}

impl RunMode {
    pub fn from_execute(execute: bool) -> Self {
        if execute { Self::Execute } else { Self::DryRun }
    }

    pub fn allows_trading(self) -> bool {
        self == Self::Execute
    }
}

pub enum PublishEvent {
    BatchStart,
    BatchEnd {
        received_at: std::time::Instant,
    },
    LiveEvent(hftbacktest::types::LiveEvent),
    /// The private account stream is unavailable. Live execution must fail closed until each
    /// configured symbol has been reconciled again.
    AccountStreamDisconnected,
    /// The public market stream is unavailable; outstanding strategy orders must be canceled.
    MarketStreamDisconnected,
    /// A symbol-level REST reconciliation is about to start. New submissions for the symbol must
    /// pause until a matching `AccountSnapshotReady` is published.
    AccountReconciliationStarted {
        symbol: String,
    },
    /// Position and strategy-owned open orders were recovered from one REST snapshot after the
    /// private stream connected or after a symbol-level reconciliation. This is the only event that
    /// may authorize execution.
    AccountSnapshotReady {
        symbol: String,
    },
    /// Binance definitively rejected a submission because the requested side would exceed an
    /// exchange-side position/capacity constraint. The order was not created, so this must not be
    /// treated as an uncertain submission. The live service may temporarily block this side while
    /// allowing the opposite side to keep reducing inventory.
    SubmissionCapacityRejected {
        symbol: String,
        side: Side,
        code: i64,
    },
    /// The venue may have accepted a submit whose response could not be confirmed. Execution for
    /// the symbol must remain latched off until a clean restart/recovery.
    ExecutionUncertain {
        symbol: String,
    },
}

/// Public market-data side of an exchange adapter.
pub trait MarketDataSource {
    fn register(&mut self, symbol: String);
    fn start_market_data(&mut self, tx: UnboundedSender<PublishEvent>);
}

/// Authenticated order/account side of an exchange adapter.
pub trait ExecutionVenue {
    fn start_account_stream(
        &self,
        instruments: Vec<TradingInstrument>,
        tx: UnboundedSender<PublishEvent>,
    );
    fn open_orders(&self, symbol: &str) -> Vec<Order>;
    fn submit(
        &self,
        symbol: String,
        order: Order,
        lot_size: f64,
        tx: UnboundedSender<PublishEvent>,
    );
    fn cancel(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>);
}

pub trait LiveConnector: MarketDataSource + ExecutionVenue {}

impl<T> LiveConnector for T where T: MarketDataSource + ExecutionVenue {}
