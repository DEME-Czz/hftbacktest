use std::{io::Error as IoError, mem};

use crate::{
    backtest::{
        data::{Data, DataPreprocess, DataSource, Field, NpyDTyped, POD, Reader},
        BacktestError,
    },
    types::Order,
};

/// Provides the order entry latency and the order response latency.
pub trait LatencyModel {
    fn entry(&mut self, timestamp: i64, order: &Order) -> i64;
    fn response(&mut self, timestamp: i64, order: &Order) -> i64;
}

#[derive(Clone)]
pub struct ConstantLatency {
    entry_latency: i64,
    response_latency: i64,
}

impl ConstantLatency {
    pub fn new(entry_latency: i64, response_latency: i64) -> Self {
        Self {
            entry_latency,
            response_latency,
        }
    }
}

impl LatencyModel for ConstantLatency {
    fn entry(&mut self, _timestamp: i64, _order: &Order) -> i64 {
        self.entry_latency
    }

    fn response(&mut self, _timestamp: i64, _order: &Order) -> i64 {
        self.response_latency
    }
}

#[repr(C, align(32))]
#[derive(Clone, Debug)]
pub struct OrderLatencyRow {
    pub req_ts: i64,
    pub exch_ts: i64,
    pub resp_ts: i64,
    pub _padding: i64,
}

unsafe impl POD for OrderLatencyRow {}

impl NpyDTyped for OrderLatencyRow {
    fn descr() -> Vec<Field> {
        vec![
            Field {
                name: "req_ts".into(),
                ty: "<i8".into(),
            },
            Field {
                name: "exch_ts".into(),
                ty: "<i8".into(),
            },
            Field {
                name: "resp_ts".into(),
                ty: "<i8".into(),
            },
            Field {
                name: "_padding".into(),
                ty: "<i8".into(),
            },
        ]
    }
}

#[derive(Clone)]
pub struct IntpOrderLatency {
    entry_rn: usize,
    resp_rn: usize,
    reader: Reader<OrderLatencyRow>,
    data: Data<OrderLatencyRow>,
    next_data: Data<OrderLatencyRow>,
}

impl IntpOrderLatency {
    pub fn build(
        data: Vec<DataSource<OrderLatencyRow>>,
        parallel_load: bool,
        latency_offset: i64,
    ) -> Result<Self, BacktestError> {
        let mut reader = if latency_offset == 0 {
            Reader::builder()
                .parallel_load(parallel_load)
                .data(data)
                .build()?
        } else {
            Reader::builder()
                .parallel_load(parallel_load)
                .data(data)
                .preprocessor(OrderLatencyAdjustment::new(latency_offset))
                .build()?
        };
        let data = match reader.next_data() {
            Ok(data) => data,
            Err(BacktestError::EndOfData) => Data::empty(),
            Err(e) => return Err(e),
        };
        let next_data = match reader.next_data() {
            Ok(data) => data,
            Err(BacktestError::EndOfData) => Data::empty(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            entry_rn: 0,
            resp_rn: 0,
            reader,
            data,
            next_data,
        })
    }

    pub fn new(data: Vec<DataSource<OrderLatencyRow>>, latency_offset: i64) -> Self {
        Self::build(data, true, latency_offset).unwrap()
    }

    fn intp(&self, x: i64, x1: i64, y1: i64, x2: i64, y2: i64) -> i64 {
        (((y2 - y1) as f64) / ((x2 - x1) as f64) * ((x - x1) as f64)) as i64 + y1
    }

    fn next_data(&mut self) -> Result<bool, BacktestError> {
        if !self.next_data.is_empty() {
            let next_data = match self.reader.next_data() {
                Ok(data) => data,
                Err(BacktestError::EndOfData) => Data::empty(),
                Err(e) => return Err(e),
            };
            let next_data = mem::replace(&mut self.next_data, next_data);
            let data = mem::replace(&mut self.data, next_data);
            self.reader.release(data);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl LatencyModel for IntpOrderLatency {
    fn entry(&mut self, timestamp: i64, _order: &Order) -> i64 {
        let first_row = &self.data[0];
        if timestamp < first_row.req_ts {
            return first_row.exch_ts - first_row.req_ts;
        }
        loop {
            let row = &self.data[self.entry_rn];
            let next_row = if self.entry_rn + 1 < self.data.len() {
                &self.data[self.entry_rn + 1]
            } else if !self.next_data.is_empty() {
                &self.next_data[0]
            } else {
                let last_row = &self.data[self.data.len() - 1];
                return last_row.exch_ts - last_row.req_ts;
            };
            if row.req_ts <= timestamp && timestamp < next_row.req_ts {
                if row.exch_ts <= 0 || next_row.exch_ts <= 0 {
                    let lat1 = row.resp_ts - row.req_ts;
                    let lat2 = next_row.resp_ts - next_row.req_ts;
                    return -self.intp(timestamp, row.req_ts, lat1, next_row.req_ts, lat2);
                }
                let lat1 = row.exch_ts - row.req_ts;
                let lat2 = next_row.exch_ts - next_row.req_ts;
                return self.intp(timestamp, row.req_ts, lat1, next_row.req_ts, lat2);
            } else if self.entry_rn == self.data.len() - 1 {
                if self.next_data().unwrap() {
                    self.entry_rn = 0;
                }
            } else {
                self.entry_rn += 1;
            }
        }
    }

    fn response(&mut self, timestamp: i64, _order: &Order) -> i64 {
        let first_row = &self.data[0];
        if timestamp < first_row.exch_ts {
            return first_row.resp_ts - first_row.exch_ts;
        }
        loop {
            let row = &self.data[self.resp_rn];
            let next_row = if self.resp_rn + 1 < self.data.len() {
                &self.data[self.resp_rn + 1]
            } else if !self.next_data.is_empty() {
                &self.next_data[0]
            } else {
                let last_row = &self.data[self.data.len() - 1];
                return last_row.resp_ts - last_row.exch_ts;
            };
            if row.exch_ts <= timestamp && timestamp < next_row.exch_ts {
                let lat1 = row.resp_ts - row.exch_ts;
                let lat2 = next_row.resp_ts - next_row.exch_ts;
                let lat = self.intp(timestamp, row.exch_ts, lat1, next_row.exch_ts, lat2);
                assert!(lat >= 0);
                return lat;
            } else if self.resp_rn == self.data.len() - 1 {
                if self.next_data().unwrap() {
                    self.resp_rn = 0;
                }
            } else {
                self.resp_rn += 1;
            }
        }
    }
}

#[derive(Clone)]
struct OrderLatencyAdjustment {
    latency_offset: i64,
}

impl OrderLatencyAdjustment {
    fn new(latency_offset: i64) -> Self {
        Self { latency_offset }
    }
}

impl DataPreprocess<OrderLatencyRow> for OrderLatencyAdjustment {
    fn preprocess(&self, data: &mut Data<OrderLatencyRow>) -> Result<(), IoError> {
        for i in 0..data.len() {
            data[i].exch_ts += self.latency_offset;
            data[i].resp_ts += self.latency_offset * 2;
        }
        Ok(())
    }
}
