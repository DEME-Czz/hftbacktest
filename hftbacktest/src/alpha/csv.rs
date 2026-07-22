use std::{
    fs::{File, OpenOptions},
    io::{self, BufWriter, Write},
    path::Path,
};

use super::{FEATURE_COUNT, LobRecord, LobSnapshot};

pub struct CsvDatasetWriter<W> {
    writer: W,
    latest_snapshot: Option<LobSnapshot>,
}

impl<W: Write> CsvDatasetWriter<W> {
    pub fn new(writer: W) -> io::Result<Self> {
        Self::initialize(writer, true)
    }

    fn initialize(mut writer: W, write_header: bool) -> io::Result<Self> {
        if write_header {
            writer.write_all(b"exchange_timestamp,mid_price")?;
            for level in 1..=FEATURE_COUNT / 4 {
                write!(
                    writer,
                    ",ask_price_{level},ask_qty_{level},bid_price_{level},bid_qty_{level}"
                )?;
            }
            writer.write_all(b"\n")?;
        }
        Ok(Self {
            writer,
            latest_snapshot: None,
        })
    }

    /// Writes a record if its order-book state differs from the last written state.
    pub fn write(&mut self, record: &LobRecord) -> io::Result<bool> {
        if self.latest_snapshot.as_ref() == Some(record.snapshot()) {
            return Ok(false);
        }

        write!(
            self.writer,
            "{},{}",
            record.exchange_timestamp(),
            record.mid_price()
        )?;
        for value in record.snapshot().features() {
            write!(self.writer, ",{value}")?;
        }
        self.writer.write_all(b"\n")?;
        self.latest_snapshot = Some(record.snapshot().clone());
        Ok(true)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl CsvDatasetWriter<BufWriter<File>> {
    /// Opens a dataset for append, writing the CSV header only for a new or empty file.
    pub fn open_append(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.as_ref())?;
        let write_header = file.metadata()?.len() == 0;
        Self::initialize(BufWriter::new(file), write_header)
    }
}
