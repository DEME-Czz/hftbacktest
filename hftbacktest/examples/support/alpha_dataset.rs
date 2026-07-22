use std::{fs::File, io::BufWriter};

use anyhow::{Context, Result};
use hftbacktest::{
    alpha::{CsvDatasetWriter, LobRecord},
    depth::HashMapMarketDepth,
};

pub struct OptionalDatasetRecorder {
    writer: Option<CsvDatasetWriter<BufWriter<File>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordStatus {
    Disabled,
    WaitingForDepth,
    Duplicate,
    Written,
}

impl OptionalDatasetRecorder {
    pub fn from_path(path: Option<&std::path::Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self { writer: None });
        };
        let writer = CsvDatasetWriter::open_append(path).with_context(|| {
            format!(
                "failed to open Alpha dataset for append at {}",
                path.display()
            )
        })?;
        Ok(Self {
            writer: Some(writer),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.writer.is_some()
    }

    pub fn record(&mut self, depth: &HashMapMarketDepth) -> Result<RecordStatus> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(RecordStatus::Disabled);
        };
        let Ok(record) = LobRecord::from_depth(depth.timestamp, depth) else {
            return Ok(RecordStatus::WaitingForDepth);
        };
        let written = writer
            .write(&record)
            .context("failed to write Alpha dataset record")?;
        Ok(if written {
            RecordStatus::Written
        } else {
            RecordStatus::Duplicate
        })
    }

    pub fn disable(&mut self) {
        self.writer = None;
    }

    pub fn flush(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush().context("failed to flush Alpha dataset")?;
        }
        Ok(())
    }
}
