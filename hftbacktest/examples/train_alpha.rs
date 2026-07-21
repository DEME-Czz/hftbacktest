use std::{fs::File, io::BufReader, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use hftbacktest::alpha::{LabelConfig, TrainingConfig, load_csv_records, train_linear_model};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "data/doge_alpha_20260721.csv")]
    input: PathBuf,
    #[arg(long, default_value = "data/doge_alpha_20260721.model.json")]
    output: PathBuf,
    #[arg(long, default_value_t = 50)]
    horizon: usize,
    #[arg(long, default_value_t = 0.0002)]
    threshold: f64,
    #[arg(long, default_value_t = 0.8)]
    train_ratio: f64,
    #[arg(long, default_value_t = 100)]
    epochs: usize,
    #[arg(long, default_value_t = 0.05)]
    learning_rate: f32,
    #[arg(long, default_value_t = 0.0001)]
    l2: f32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let records = load_csv_records(BufReader::new(
        File::open(&args.input).with_context(|| format!("open {}", args.input.display()))?,
    ))?;
    let labels = LabelConfig::new(args.horizon, args.threshold)?;
    let training = TrainingConfig::new(args.train_ratio, args.epochs, args.learning_rate, args.l2)?;
    let (model, report) = train_linear_model(&records, labels, training)?;

    std::fs::write(&args.output, model.to_json()?)
        .with_context(|| format!("write {}", args.output.display()))?;
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
    println!("model={}", args.output.display());
    Ok(())
}
