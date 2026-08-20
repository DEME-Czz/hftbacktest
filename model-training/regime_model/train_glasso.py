from __future__ import annotations

import argparse
import csv
import json
import math
from datetime import UTC, datetime
from pathlib import Path

from .calibration import calibrate_temperature
from .fista import fit_group_lasso
from .schema import FEATURE_GROUPS, FEATURE_NAMES, feature_schema_hash


def load_csv(path: Path) -> tuple[list[list[float]], list[int], list[str]]:
    rows, labels, timestamps = [], [], []
    label_map = {"UP": 0, "SIDEWAYS": 1, "DOWN": 2, "0": 0, "1": 1, "2": 2}
    with path.open(newline="") as source:
        reader = csv.DictReader(source)
        missing = set(FEATURE_NAMES) - set(reader.fieldnames or ())
        if missing or "label" not in (reader.fieldnames or ()) or "timestamp" not in (reader.fieldnames or ()):
            raise ValueError(f"missing columns: {sorted(missing | {'label', 'timestamp'} - set(reader.fieldnames or ())) }")
        for record in reader:
            row = [float(record[name]) for name in FEATURE_NAMES]
            if not all(math.isfinite(value) for value in row):
                raise ValueError("features must be finite")
            rows.append(row)
            labels.append(label_map[record["label"].upper()])
            timestamps.append(record["timestamp"])
    return rows, labels, timestamps


def standardize(rows: list[list[float]]) -> tuple[list[list[float]], list[float], list[float]]:
    count, width = len(rows), len(rows[0])
    mean = [sum(row[j] for row in rows) / count for j in range(width)]
    std = [math.sqrt(sum((row[j] - mean[j]) ** 2 for row in rows) / count) for j in range(width)]
    std = [value if value > 1e-12 else 1.0 for value in std]
    return [[(value - mean[j]) / std[j] for j, value in enumerate(row)] for row in rows], mean, std


def apply_standardization(rows: list[list[float]], mean: list[float], std: list[float]) -> list[list[float]]:
    return [
        [max(-8.0, min(8.0, (value - mean[j]) / std[j])) for j, value in enumerate(row)]
        for row in rows
    ]


def main() -> None:
    parser = argparse.ArgumentParser(description="Train the regime multinomial Group-LASSO model")
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--validation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--regularization", type=float, default=0.01)
    parser.add_argument("--prediction-horizon-ms", type=int, default=60_000)
    parser.add_argument("--version", default=f"regime_glasso_{datetime.now(UTC):%Y%m%d}")
    args = parser.parse_args()

    rows, labels, timestamps = load_csv(args.input)
    _, mean, std = standardize(rows)
    rows = apply_standardization(rows, mean, std)
    validation_rows, validation_labels, _ = load_csv(args.validation)
    validation_rows = apply_standardization(validation_rows, mean, std)
    result = fit_group_lasso(rows, labels, args.regularization)
    validation_logits = [
        (
            result.intercept_up + sum(a * b for a, b in zip(result.coef_up, row, strict=True)),
            0.0,
            result.intercept_down + sum(a * b for a, b in zip(result.coef_down, row, strict=True)),
        )
        for row in validation_rows
    ]
    temperature = calibrate_temperature(validation_logits, validation_labels)
    model = {
        "model_type": "multinomial_group_lasso", "version": args.version,
        "sample_interval_ms": 1000, "prediction_horizon_ms": args.prediction_horizon_ms,
        "feature_schema_hash": feature_schema_hash(), "features": list(FEATURE_NAMES),
        "groups": {name: list(indices) for name, indices in FEATURE_GROUPS.items()},
        "mean": mean, "std": std, "intercept_up": result.intercept_up,
        "intercept_down": result.intercept_down, "coef_up": list(result.coef_up),
        "coef_down": list(result.coef_down), "temperature": temperature,
        "training_start": timestamps[0], "training_end": timestamps[-1],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(model, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
