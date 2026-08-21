use std::{
    fmt::Debug,
    sync::{Arc, Mutex},
};

use hftbacktest::types::Order;
use tokio::sync::mpsc::UnboundedSender;

/// A message will be received by the publisher thread and then published to the bots.
pub enum PublishEvent {
    BatchStart(String),
    BatchEnd(String),
    LiveEvent(hftbacktest::types::LiveEvent),
    RegisterInstrument {
        id: u64,
        symbol: String,
        tick_size: f64,
        lot_size: f64,
    },
}

/// Provides a build function for the Connector.
pub trait ConnectorBuilder {
    type Error: Debug;

    fn build_from(config: &str) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

/// Provides an interface for connecting with an exchange or broker for a live bot.
pub trait Connector {
    fn register(&mut self, symbol: String);
    fn order_manager(&self) -> Arc<Mutex<dyn GetOrders + Send + 'static>>;
    fn run(&mut self, tx: UnboundedSender<PublishEvent>);
    fn submit(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>);
    fn cancel(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>);
}

pub trait GetOrders {
    fn orders(&self, symbol: Option<String>) -> Vec<Order>;
}
