use std::{
    fmt::Debug,
    sync::{Arc, Mutex},
};

use hftbacktest::types::Order;
use tokio::sync::mpsc::UnboundedSender;

pub enum PublishEvent {
    BatchStart(u64),
    BatchEnd(u64),
    LiveEvent(hftbacktest::types::LiveEvent),
    RegisterInstrument {
        id: u64,
        symbol: String,
        tick_size: f64,
        lot_size: f64,
    },
}

pub trait ConnectorBuilder {
    type Error: Debug;

    fn build_from(config: &str) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

pub trait Connector {
    fn register(&mut self, symbol: String);
    fn order_manager(&self) -> Arc<Mutex<dyn GetOrders + Send + 'static>>;
    fn run(&mut self, tx: UnboundedSender<PublishEvent>);
    fn submit(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>);
    fn modify(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>);
    fn cancel(&self, symbol: String, order: Order, tx: UnboundedSender<PublishEvent>);
}

pub trait GetOrders {
    fn orders(&self, symbol: Option<String>) -> Vec<Order>;
}
