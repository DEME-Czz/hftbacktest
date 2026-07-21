use std::io::BufRead;

use thiserror::Error;

use super::{FEATURE_COUNT, LobRecord, LobSnapshot, RecordError, SnapshotError};

const COLUMN_COUNT: usize = FEATURE_COUNT + 2;

pub fn load_csv_records(mut reader: impl BufRead) -> Result<Vec<LobRecord>, DatasetError> {
    let mut content = String::new();
    reader.read_to_string(&mut content)?;
    let has_partial_tail = !content.ends_with('\n');
    let lines: Vec<_> = content.lines().collect();
    let header = lines.first().ok_or(DatasetError::MissingHeader)?;
    if header.split(',').count() != COLUMN_COUNT
        || !header.starts_with("exchange_timestamp,mid_price,ask_price_1,ask_qty_1")
    {
        return Err(DatasetError::InvalidHeader);
    }

    let mut records = Vec::new();
    for (index, line) in lines.iter().skip(1).enumerate() {
        let line_number = index + 2;
        if line.trim().is_empty() {
            continue;
        }
        let columns: Vec<_> = line.split(',').collect();
        if columns.len() != COLUMN_COUNT {
            if has_partial_tail && line_number == lines.len() {
                break;
            }
            return Err(DatasetError::ColumnCount {
                line: line_number,
                found: columns.len(),
            });
        }
        let timestamp = columns[0]
            .parse::<i64>()
            .map_err(|_| DatasetError::InvalidValue {
                line: line_number,
                column: 1,
            })?;
        let mid = parse_f64(columns[1], line_number, 2)?;
        let mut features = [0.0_f32; FEATURE_COUNT];
        for (feature, value) in features.iter_mut().zip(&columns[2..]) {
            *feature = value
                .parse::<f32>()
                .map_err(|_| DatasetError::InvalidValue {
                    line: line_number,
                    column: 3,
                })?;
        }
        records.push(LobRecord::new(timestamp, mid, LobSnapshot::new(features)?)?);
    }
    if records.is_empty() {
        return Err(DatasetError::Empty);
    }
    Ok(records)
}

fn parse_f64(value: &str, line: usize, column: usize) -> Result<f64, DatasetError> {
    value
        .parse::<f64>()
        .map_err(|_| DatasetError::InvalidValue { line, column })
}

#[derive(Debug, Error)]
pub enum DatasetError {
    #[error("dataset has no header")]
    MissingHeader,
    #[error("dataset header does not match the 42-column Alpha schema")]
    InvalidHeader,
    #[error("line {line} has {found} columns; expected {COLUMN_COUNT}")]
    ColumnCount { line: usize, found: usize },
    #[error("line {line}, column {column} is not a valid number")]
    InvalidValue { line: usize, column: usize },
    #[error("dataset contains no records")]
    Empty,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Record(#[from] RecordError),
}
