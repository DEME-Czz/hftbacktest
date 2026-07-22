use std::{fs::File, io::BufReader};

use anyhow::{Context, Result};
use clap::Parser;
use hftbacktest::alpha::{LabelConfig, TrainingConfig, load_csv_records, train_linear_model};

#[derive(Parser)]
struct Args {
    /// TOML containing every training parameter.
    config: String,
}

#[path = "support/train_alpha_config.rs"]
mod train_alpha_config;

fn main() -> Result<()> {
    let args = Args::parse();
    let config = train_alpha_config::TrainAlphaConfig::load(&args.config)?;
    let records = load_csv_records(BufReader::new(
        File::open(&config.input).with_context(|| format!("open {}", config.input.display()))?,
    ))?;
    let labels = LabelConfig::new(config.horizon, config.threshold)?;
    let training = TrainingConfig::new(
        config.train_ratio,
        config.epochs,
        config.learning_rate,
        config.l2,
    )?;
    let (model, report) = train_linear_model(&records, labels, training)?;

    std::fs::write(&config.output, model.to_json()?)
        .with_context(|| format!("write {}", config.output.display()))?;
    println!(
        "records={} train={} validation={}",
        report.records, report.train_samples, report.validation_samples
    );
    println!(
        "train_classes(down,flat,up)={:?}",
        report.train_class_counts
    );
    println!(
        "validation_classes(down,flat,up)={:?}",
        report.validation_class_counts
    );
    println!(
        "confusion(actual rows, predicted columns)={:?}",
        report.confusion
    );
    println!("validation_accuracy={:.4}", report.validation_accuracy);
    println!("model={}", config.output.display());
    Ok(())
}
