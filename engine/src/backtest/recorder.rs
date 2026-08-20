use std::{
    fs::File,
    io::{BufWriter, Error, Write},
    path::Path,
};

use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{
    backtest::data::{Field, NpyDTyped, POD, write_npy},
    depth::MarketDepth,
    types::{Bot, Recorder},
};

#[repr(C)]
struct Record {
    timestamp: i64,
    price: f64,
    position: f64,
    balance: f64,
    fee: f64,
    num_trades: i64,
    trading_volume: f64,
    trading_value: f64,
}

unsafe impl POD for Record {}

impl NpyDTyped for Record {
    fn descr() -> Vec<Field> {
        let endian = if cfg!(target_endian = "little") { "<" } else { ">" };
        [
            ("timestamp", "i8"),
            ("price", "f8"),
            ("position", "f8"),
            ("balance", "f8"),
            ("fee", "f8"),
            ("num_trades", "i8"),
            ("trading_volume", "f8"),
            ("trading_value", "f8"),
        ]
        .into_iter()
        .map(|(name, ty)| Field {
            name: name.to_string(),
            ty: format!("{endian}{ty}"),
        })
        .collect()
    }
}

pub struct BacktestRecorder {
    values: Vec<Vec<Record>>,
}

impl Recorder for BacktestRecorder {
    type Error = Error;

    fn record<MD, I>(&mut self, hbt: &I) -> Result<(), Self::Error>
    where
        MD: MarketDepth,
        I: Bot<MD>,
    {
        let timestamp = hbt.current_timestamp();
        for asset_no in 0..hbt.num_assets() {
            let depth = hbt.depth(asset_no);
            let mid_price = (depth.best_bid() + depth.best_ask()) / 2.0;
            let state_values = hbt.state_values(asset_no);
            self.values[asset_no].push(Record {
                timestamp,
                price: mid_price,
                balance: state_values.balance,
                position: state_values.position,
                fee: state_values.fee,
                trading_volume: state_values.trading_volume,
                trading_value: state_values.trading_value,
                num_trades: state_values.num_trades,
            });
        }
        Ok(())
    }
}

impl BacktestRecorder {
    pub fn new<I, MD>(hbt: &I) -> Self
    where
        MD: MarketDepth,
        I: Bot<MD>,
    {
        Self {
            values: (0..hbt.num_assets()).map(|_| Vec::new()).collect(),
        }
    }

    pub fn to_csv<Prefix, P>(&self, prefix: Prefix, path: P) -> Result<(), Error>
    where
        Prefix: AsRef<str>,
        P: AsRef<Path>,
    {
        let prefix = prefix.as_ref();
        for (asset_no, values) in self.values.iter().enumerate() {
            let file_path = path.as_ref().join(format!("{prefix}{asset_no}.csv"));
            let mut file = BufWriter::new(File::create(file_path)?);
            writeln!(
                file,
                "timestamp,balance,position,fee,trading_volume,trading_value,num_trades,price",
            )?;
            for Record {
                timestamp,
                balance,
                position,
                fee,
                trading_volume,
                trading_value,
                num_trades,
                price: mid_price,
            } in values
            {
                writeln!(
                    file,
                    "{timestamp},{balance},{position},{fee},{trading_volume},{trading_value},{num_trades},{mid_price}"
                )?;
            }
        }
        Ok(())
    }

    pub fn to_npz<P>(&self, path: P) -> Result<(), Error>
    where
        P: AsRef<Path>,
    {
        let file = File::create(path)?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::DEFLATE)
            .compression_level(Some(9));
        for (asset_no, values) in self.values.iter().enumerate() {
            zip.start_file(format!("{asset_no}.npy"), options)?;
            write_npy(&mut zip, values)?;
        }
        zip.finish()?;
        Ok(())
    }
}
