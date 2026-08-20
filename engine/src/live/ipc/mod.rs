use std::time::Duration;

use crate::{
    live::{BotError, Instrument},
    prelude::BuildError,
    types::{LiveEvent, LiveRequest},
};

/// Broadcast target used by the live event protocol.
pub const TO_ALL: u64 = 0;

/// Transport abstraction for a live bot.
///
/// The engine deliberately provides no concrete IPC/network implementation. A runtime crate may
/// implement this trait with in-process channels, shared memory, TCP, or another transport without
/// pulling that transport into the backtest engine.
pub trait Channel {
    fn build<MD>(instruments: &[Instrument<MD>]) -> Result<Self, BuildError>
    where
        Self: Sized;

    fn recv_timeout(&mut self, id: u64, timeout: Duration) -> Result<(usize, LiveEvent), BotError>;

    fn send(&mut self, id: u64, inst_no: usize, request: LiveRequest) -> Result<(), BotError>;
}
