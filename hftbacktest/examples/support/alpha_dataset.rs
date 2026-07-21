use std::{
    env,
    fs::{File, OpenOptions},
    io::BufWriter,
};

use anyhow::{Context, Result};
use hftbacktest::{
    alpha::{CsvDatasetWriter, LobRecord},
    depth::HashMapMarketDepth,
};

const DATASET_PATH_ENV: &str = "HFT_ALPHA_DATASET_PATH";

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
    pub fn from_env() -> Result<Self> {
        let Some(path) = env::var_os(DATASET_PATH_ENV) else {
            return Ok(Self { writer: None });
        };
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| {
                format!(
                    "failed to create Alpha dataset at {}; choose a new path because existing files are never overwritten",
                    path.to_string_lossy()
                )
            })?;
        let writer = CsvDatasetWriter::new(BufWriter::new(file))
            .context("failed to initialize Alpha dataset CSV")?;
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
