use std::{
    collections::HashMap,
    io::Error as IoError,
    ops::{Deref, DerefMut},
};

pub use data::DataSource;
use data::Reader;
use models::FeeModel;
use thiserror::Error;

use crate::{
    backtest::{
        assettype::AssetType,
        data::{Data, FeedLatencyAdjustment, NpyDTyped},
        evs::{EventIntentKind, EventSet},
        models::{LatencyModel, QueueModel},
        order::order_bus,
        proc::{Local, LocalProcessor, NoPartialFillExchange, PartialFillExchange, Processor},
        state::State,
    },
    depth::{L2MarketDepth, MarketDepth},
    prelude::{
        Bot, OrdType, Order, OrderId, OrderRequest, Side, StateValues, TimeInForce,
        UNTIL_END_OF_DATA, WaitOrderResponse,
    },
    types::{BuildError, ElapseResult, Event},
};

pub mod assettype;
pub mod data;
mod evs;
pub mod models;
pub mod order;
pub mod proc;
pub mod recorder;
pub mod state;

#[derive(Error, Debug)]
pub enum BacktestError {
    #[error("Order related to a given order id already exists")]
    OrderIdExist,
    #[error("Order request is in process")]
    OrderRequestInProcess,
    #[error("Order not found")]
    OrderNotFound,
    #[error("order request is invalid")]
    InvalidOrderRequest,
    #[error("order status is invalid to proceed the request")]
    InvalidOrderStatus,
    #[error("end of data")]
    EndOfData,
    #[error("data error: {0:?}")]
    DataError(#[from] IoError),
}

pub struct Asset<L: ?Sized, E: ?Sized, D: NpyDTyped + Clone> {
    pub local: Box<L>,
    pub exch: Box<E>,
    pub reader: Reader<D>,
}

impl<L, E, D: NpyDTyped + Clone> Asset<L, E, D> {
    pub fn new(local: L, exch: E, reader: Reader<D>) -> Self {
        Self { local: Box::new(local), exch: Box::new(exch), reader }
    }

    pub fn l2_builder<LM, AT, QM, MD, FM>() -> L2AssetBuilder<LM, AT, QM, MD, FM>
    where
        AT: AssetType + Clone + 'static,
        MD: MarketDepth + L2MarketDepth + 'static,
        QM: QueueModel<MD> + 'static,
        LM: LatencyModel + Clone + 'static,
        FM: FeeModel + Clone + 'static,
    {
        L2AssetBuilder::new()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum ExchangeKind {
    NoPartialFillExchange,
    #[default]
    PartialFillExchange,
}

pub struct L2AssetBuilder<LM, AT, QM, MD, FM> {
    latency_model: Option<LM>,
    asset_type: Option<AT>,
    data: Vec<DataSource<Event>>,
    parallel_load: bool,
    latency_offset: i64,
    fee_model: Option<FM>,
    exch_kind: ExchangeKind,
    last_trades_cap: usize,
    queue_model: Option<QM>,
    depth_builder: Option<Box<dyn Fn() -> MD>>,
}

impl<LM, AT, QM, MD, FM> L2AssetBuilder<LM, AT, QM, MD, FM>
where
    AT: AssetType + Clone + 'static,
    MD: MarketDepth + L2MarketDepth + 'static,
    QM: QueueModel<MD> + 'static,
    LM: LatencyModel + Clone + 'static,
    FM: FeeModel + Clone + 'static,
{
    pub fn new() -> Self {
        Self {
            latency_model: None,
            asset_type: None,
            data: vec![],
            parallel_load: false,
            latency_offset: 0,
            fee_model: None,
            exch_kind: ExchangeKind::PartialFillExchange,
            last_trades_cap: 0,
            queue_model: None,
            depth_builder: None,
        }
    }

    pub fn data(self, data: Vec<DataSource<Event>>) -> Self { Self { data, ..self } }
    pub fn parallel_load(self, parallel_load: bool) -> Self { Self { parallel_load, ..self } }
    pub fn latency_offset(self, latency_offset: i64) -> Self { Self { latency_offset, ..self } }
    pub fn latency_model(self, latency_model: LM) -> Self {
        Self { latency_model: Some(latency_model), ..self }
    }
    pub fn asset_type(self, asset_type: AT) -> Self {
        Self { asset_type: Some(asset_type), ..self }
    }
    pub fn fee_model(self, fee_model: FM) -> Self {
        Self { fee_model: Some(fee_model), ..self }
    }
    pub fn exchange(self, exch_kind: ExchangeKind) -> Self { Self { exch_kind, ..self } }
    pub fn last_trades_capacity(self, capacity: usize) -> Self {
        Self { last_trades_cap: capacity, ..self }
    }
    pub fn queue_model(self, queue_model: QM) -> Self {
        Self { queue_model: Some(queue_model), ..self }
    }
    pub fn depth<Builder>(self, builder: Builder) -> Self
    where
        Builder: Fn() -> MD + 'static,
    {
        Self { depth_builder: Some(Box::new(builder)), ..self }
    }

    pub fn build(self) -> Result<Asset<dyn LocalProcessor<MD>, dyn Processor, Event>, BuildError> {
        let reader = if self.latency_offset == 0 {
            Reader::builder()
                .parallel_load(self.parallel_load)
                .data(self.data)
                .build()
                .map_err(|err| BuildError::Error(err.into()))?
        } else {
            Reader::builder()
                .parallel_load(self.parallel_load)
                .data(self.data)
                .preprocessor(FeedLatencyAdjustment::new(self.latency_offset))
                .build()
                .map_err(|err| BuildError::Error(err.into()))?
        };

        let create_depth = self.depth_builder.as_ref().ok_or(BuildError::BuilderIncomplete("depth"))?;
        let order_latency = self.latency_model.clone().ok_or(BuildError::BuilderIncomplete("order_latency"))?;
        let asset_type = self.asset_type.clone().ok_or(BuildError::BuilderIncomplete("asset_type"))?;
        let fee_model = self.fee_model.clone().ok_or(BuildError::BuilderIncomplete("fee_model"))?;
        let (order_e2l, order_l2e) = order_bus(order_latency);
        let local = Local::new(
            create_depth(),
            State::new(asset_type, fee_model),
            self.last_trades_cap,
            order_l2e,
        );
        let queue_model = self.queue_model.ok_or(BuildError::BuilderIncomplete("queue_model"))?;
        let asset_type = self.asset_type.clone().ok_or(BuildError::BuilderIncomplete("asset_type"))?;
        let fee_model = self.fee_model.clone().ok_or(BuildError::BuilderIncomplete("fee_model"))?;

        let exch: Box<dyn Processor> = match self.exch_kind {
            ExchangeKind::NoPartialFillExchange => Box::new(NoPartialFillExchange::new(
                create_depth(), State::new(asset_type, fee_model), queue_model, order_e2l,
            )),
            ExchangeKind::PartialFillExchange => Box::new(PartialFillExchange::new(
                create_depth(), State::new(asset_type, fee_model), queue_model, order_e2l,
            )),
        };

        Ok(Asset { local: Box::new(local), exch, reader })
    }
}

impl<LM, AT, QM, MD, FM> Default for L2AssetBuilder<LM, AT, QM, MD, FM>
where
    AT: AssetType + Clone + 'static,
    MD: MarketDepth + L2MarketDepth + 'static,
    QM: QueueModel<MD> + 'static,
    LM: LatencyModel + Clone + 'static,
    FM: FeeModel + Clone + 'static,
{
    fn default() -> Self { Self::new() }
}

pub struct BacktestBuilder<MD> {
    local: Vec<BacktestProcessorState<Box<dyn LocalProcessor<MD>>>>,
    exch: Vec<BacktestProcessorState<Box<dyn Processor>>>,
}

impl<MD> BacktestBuilder<MD> {
    pub fn add_asset(self, asset: Asset<dyn LocalProcessor<MD>, dyn Processor, Event>) -> Self {
        let mut this = self;
        this.local.push(BacktestProcessorState::new(asset.local, asset.reader.clone()));
        this.exch.push(BacktestProcessorState::new(asset.exch, asset.reader));
        this
    }

    pub fn build(self) -> Result<Backtest<MD>, BuildError> {
        Ok(Backtest {
            cur_ts: i64::MAX,
            evs: EventSet::new(self.local.len()),
            local: self.local,
            exch: self.exch,
        })
    }
}

pub struct Backtest<MD> {
    cur_ts: i64,
    evs: EventSet,
    local: Vec<BacktestProcessorState<Box<dyn LocalProcessor<MD>>>>,
    exch: Vec<BacktestProcessorState<Box<dyn Processor>>>,
}

impl<P: Processor> Deref for BacktestProcessorState<P> {
    type Target = P;
    fn deref(&self) -> &Self::Target { &self.processor }
}
impl<P: Processor> DerefMut for BacktestProcessorState<P> {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.processor }
}

pub struct BacktestProcessorState<P: Processor> {
    data: Data<Event>,
    processor: P,
    reader: Reader<Event>,
    row: Option<usize>,
}

impl<P: Processor> BacktestProcessorState<P> {
    fn new(processor: P, reader: Reader<Event>) -> Self {
        Self { data: Data::empty(), processor, reader, row: None }
    }

    fn next_row(&mut self) -> Result<usize, BacktestError> {
        if self.row.is_none() { let _ = self.advance()?; }
        self.row.ok_or(BacktestError::EndOfData)
    }

    fn advance(&mut self) -> Result<i64, BacktestError> {
        loop {
            let start = self.row.map(|row| row + 1).unwrap_or(0);
            for row in start..self.data.len() {
                if let Some(ts) = self.processor.event_seen_timestamp(&self.data[row]) {
                    self.row = Some(row);
                    return Ok(ts);
                }
            }
            let next = self.reader.next_data()?;
            self.reader.release(std::mem::replace(&mut self.data, next));
            self.row = None;
        }
    }
}

impl<MD> Backtest<MD>
where
    MD: MarketDepth,
{
    pub fn builder() -> BacktestBuilder<MD> {
        BacktestBuilder { local: vec![], exch: vec![] }
    }

    pub fn new(
        local: Vec<Box<dyn LocalProcessor<MD>>>,
        exch: Vec<Box<dyn Processor>>,
        reader: Vec<Reader<Event>>,
    ) -> Self {
        let num_assets = local.len();
        assert_eq!(exch.len(), num_assets);
        assert_eq!(reader.len(), num_assets);
        let local = local
            .into_iter()
            .zip(reader.iter())
            .map(|(processor, reader)| BacktestProcessorState::new(processor, reader.clone()))
            .collect();
        let exch = exch
            .into_iter()
            .zip(reader.iter())
            .map(|(processor, reader)| BacktestProcessorState::new(processor, reader.clone()))
            .collect();
        Self { local, exch, cur_ts: i64::MAX, evs: EventSet::new(num_assets) }
    }

    fn initialize_evs(&mut self) -> Result<(), BacktestError> {
        for (asset_no, local) in self.local.iter_mut().enumerate() {
            match local.advance() {
                Ok(ts) => self.evs.update_local_data(asset_no, ts),
                Err(BacktestError::EndOfData) => self.evs.invalidate_local_data(asset_no),
                Err(error) => return Err(error),
            }
        }
        for (asset_no, exch) in self.exch.iter_mut().enumerate() {
            match exch.advance() {
                Ok(ts) => self.evs.update_exch_data(asset_no, ts),
                Err(BacktestError::EndOfData) => self.evs.invalidate_exch_data(asset_no),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn goto_end(&mut self) -> Result<ElapseResult, BacktestError> {
        if self.cur_ts == i64::MAX {
            self.initialize_evs()?;
            match self.evs.next() {
                Some(event) => self.cur_ts = event.timestamp,
                None => return Ok(ElapseResult::EndOfData),
            }
        }
        self.goto::<false>(UNTIL_END_OF_DATA, WaitOrderResponse::None)
    }

    fn goto<const WAIT_NEXT_FEED: bool>(
        &mut self,
        timestamp: i64,
        wait_order_response: WaitOrderResponse,
    ) -> Result<ElapseResult, BacktestError> {
        let mut result = ElapseResult::Ok;
        let mut target = timestamp;
        for (asset_no, local) in self.local.iter().enumerate() {
            self.evs.update_exch_order(asset_no, local.earliest_send_order_timestamp());
            self.evs.update_local_order(asset_no, local.earliest_recv_order_timestamp());
        }

        loop {
            let Some(intent) = self.evs.next() else { return Ok(ElapseResult::EndOfData); };
            if intent.timestamp > target {
                self.cur_ts = target;
                return Ok(result);
            }
            match intent.kind {
                EventIntentKind::LocalData => {
                    let local = unsafe { self.local.get_unchecked_mut(intent.asset_no) };
                    let next = local.next_row().and_then(|row| {
                        local.processor.process(&local.data[row])?;
                        local.advance()
                    });
                    match next {
                        Ok(ts) => self.evs.update_local_data(intent.asset_no, ts),
                        Err(BacktestError::EndOfData) => self.evs.invalidate_local_data(intent.asset_no),
                        Err(error) => return Err(error),
                    }
                    if WAIT_NEXT_FEED {
                        target = intent.timestamp;
                        result = ElapseResult::MarketFeed;
                    }
                }
                EventIntentKind::LocalOrder => {
                    let local = unsafe { self.local.get_unchecked_mut(intent.asset_no) };
                    let wait_id = match wait_order_response {
                        WaitOrderResponse::Specified { asset_no, order_id } if asset_no == intent.asset_no => Some(order_id),
                        _ => None,
                    };
                    if local.process_recv_order(intent.timestamp, wait_id)?
                        || wait_order_response == WaitOrderResponse::Any
                    {
                        target = intent.timestamp;
                        if WAIT_NEXT_FEED { result = ElapseResult::OrderResponse; }
                    }
                    self.evs.update_local_order(intent.asset_no, local.earliest_recv_order_timestamp());
                }
                EventIntentKind::ExchData => {
                    let exch = unsafe { self.exch.get_unchecked_mut(intent.asset_no) };
                    let next = exch.next_row().and_then(|row| {
                        exch.processor.process(&exch.data[row])?;
                        exch.advance()
                    });
                    match next {
                        Ok(ts) => self.evs.update_exch_data(intent.asset_no, ts),
                        Err(BacktestError::EndOfData) => self.evs.invalidate_exch_data(intent.asset_no),
                        Err(error) => return Err(error),
                    }
                    self.evs.update_local_order(intent.asset_no, exch.earliest_send_order_timestamp());
                }
                EventIntentKind::ExchOrder => {
                    let exch = unsafe { self.exch.get_unchecked_mut(intent.asset_no) };
                    let _ = exch.process_recv_order(intent.timestamp, None)?;
                    self.evs.update_exch_order(intent.asset_no, exch.earliest_recv_order_timestamp());
                    self.evs.update_local_order(intent.asset_no, exch.earliest_send_order_timestamp());
                }
            }
        }
    }
}

impl<MD> Bot<MD> for Backtest<MD>
where
    MD: MarketDepth,
{
    type Error = BacktestError;

    fn current_timestamp(&self) -> i64 { self.cur_ts }
    fn num_assets(&self) -> usize { self.local.len() }
    fn position(&self, asset_no: usize) -> f64 { self.local[asset_no].position() }
    fn state_values(&self, asset_no: usize) -> &StateValues { self.local[asset_no].state_values() }
    fn depth(&self, asset_no: usize) -> &MD { self.local[asset_no].depth() }
    fn last_trades(&self, asset_no: usize) -> &[Event] { self.local[asset_no].last_trades() }

    fn clear_last_trades(&mut self, asset_no: Option<usize>) {
        match asset_no {
            Some(asset_no) => self.local[asset_no].clear_last_trades(),
            None => self.local.iter_mut().for_each(|local| local.clear_last_trades()),
        }
    }

    fn orders(&self, asset_no: usize) -> &HashMap<OrderId, Order> { self.local[asset_no].orders() }

    fn submit_buy_order(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        price: f64,
        qty: f64,
        time_in_force: TimeInForce,
        order_type: OrdType,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.local[asset_no].submit_order(
            order_id, Side::Buy, price, qty, order_type, time_in_force, self.cur_ts,
        )?;
        if wait {
            self.goto::<false>(UNTIL_END_OF_DATA, WaitOrderResponse::Specified { asset_no, order_id })
        } else {
            Ok(ElapseResult::Ok)
        }
    }

    fn submit_sell_order(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        price: f64,
        qty: f64,
        time_in_force: TimeInForce,
        order_type: OrdType,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.local[asset_no].submit_order(
            order_id, Side::Sell, price, qty, order_type, time_in_force, self.cur_ts,
        )?;
        if wait {
            self.goto::<false>(UNTIL_END_OF_DATA, WaitOrderResponse::Specified { asset_no, order_id })
        } else {
            Ok(ElapseResult::Ok)
        }
    }

    fn submit_order(
        &mut self,
        asset_no: usize,
        order: OrderRequest,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.local[asset_no].submit_order(
            order.order_id, order.side, order.price, order.qty, order.order_type,
            order.time_in_force, self.cur_ts,
        )?;
        if wait {
            self.goto::<false>(
                UNTIL_END_OF_DATA,
                WaitOrderResponse::Specified { asset_no, order_id: order.order_id },
            )
        } else {
            Ok(ElapseResult::Ok)
        }
    }

    fn modify(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        price: f64,
        qty: f64,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.local[asset_no].modify(order_id, price, qty, self.cur_ts)?;
        if wait {
            self.goto::<false>(UNTIL_END_OF_DATA, WaitOrderResponse::Specified { asset_no, order_id })
        } else {
            Ok(ElapseResult::Ok)
        }
    }

    fn cancel(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.local[asset_no].cancel(order_id, self.cur_ts)?;
        if wait {
            self.goto::<false>(UNTIL_END_OF_DATA, WaitOrderResponse::Specified { asset_no, order_id })
        } else {
            Ok(ElapseResult::Ok)
        }
    }

    fn clear_inactive_orders(&mut self, asset_no: Option<usize>) {
        match asset_no {
            Some(asset_no) => self.local[asset_no].clear_inactive_orders(),
            None => self.local.iter_mut().for_each(|local| local.clear_inactive_orders()),
        }
    }

    fn wait_order_response(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        timeout: i64,
    ) -> Result<ElapseResult, Self::Error> {
        self.goto::<false>(
            self.cur_ts + timeout,
            WaitOrderResponse::Specified { asset_no, order_id },
        )
    }

    fn wait_next_feed(
        &mut self,
        include_order_resp: bool,
        timeout: i64,
    ) -> Result<ElapseResult, Self::Error> {
        if self.cur_ts == i64::MAX {
            self.initialize_evs()?;
            match self.evs.next() {
                Some(event) => self.cur_ts = event.timestamp,
                None => return Ok(ElapseResult::EndOfData),
            }
        }
        let wait = if include_order_resp { WaitOrderResponse::Any } else { WaitOrderResponse::None };
        self.goto::<true>(self.cur_ts + timeout, wait)
    }

    fn elapse(&mut self, duration: i64) -> Result<ElapseResult, Self::Error> {
        if self.cur_ts == i64::MAX {
            self.initialize_evs()?;
            match self.evs.next() {
                Some(event) => self.cur_ts = event.timestamp,
                None => return Ok(ElapseResult::EndOfData),
            }
        }
        self.goto::<false>(self.cur_ts + duration, WaitOrderResponse::None)
    }

    fn elapse_bt(&mut self, duration: i64) -> Result<ElapseResult, Self::Error> { self.elapse(duration) }
    fn close(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn feed_latency(&self, asset_no: usize) -> Option<(i64, i64)> { self.local[asset_no].feed_latency() }
    fn order_latency(&self, asset_no: usize) -> Option<(i64, i64, i64)> { self.local[asset_no].order_latency() }
}
