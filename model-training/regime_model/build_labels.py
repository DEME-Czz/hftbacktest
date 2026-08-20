from __future__ import annotations

import bisect
import argparse
import csv
import math
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from .schema import FEATURE_NAMES


@dataclass(frozen=True)
class LabelConfig:
    prediction_horizon_ms: int = 60_000
    volatility_multiplier: float = 0.80
    grid_multiplier: float = 2.00
    cost_floor_bps: float = 2.00


@dataclass(frozen=True)
class Label:
    index: int
    future_index: int
    value: int  # 0=UP, 1=SIDEWAYS, 2=DOWN
    future_return: float
    threshold: float


def build_labels(
    timestamps_ms: Sequence[int],
    mids: Sequence[float],
    relative_grid_intervals: Sequence[float],
    config: LabelConfig = LabelConfig(),
) -> list[Label]:
    """Create causal dynamic-threshold labels; only the target return uses future data."""
    if not (len(timestamps_ms) == len(mids) == len(relative_grid_intervals)):
        raise ValueError("input sequences must be aligned")
    if any(b <= a for a, b in zip(timestamps_ms, timestamps_ms[1:])):
        raise ValueError("timestamps must be strictly increasing")
    if any(not math.isfinite(mid) or mid <= 0.0 for mid in mids):
        raise ValueError("mids must be finite and positive")
    if config.prediction_horizon_ms <= 0:
        raise ValueError("prediction horizon must be positive")

    log_returns = [0.0]
    log_returns.extend(math.log(new / old) for old, new in zip(mids, mids[1:]))
    labels: list[Label] = []
    for index, (timestamp, mid, grid) in enumerate(
        zip(timestamps_ms, mids, relative_grid_intervals, strict=True)
    ):
        future_index = bisect.bisect_left(
            timestamps_ms, timestamp + config.prediction_horizon_ms, lo=index + 1
        )
        if future_index >= len(mids):
            break
        history_start = bisect.bisect_left(
            timestamps_ms, timestamp - config.prediction_horizon_ms, hi=index + 1
        )
        # Realized volatility is calculated entirely from t and earlier observations.
        volatility = math.sqrt(sum(value * value for value in log_returns[history_start:index + 1]))
        threshold = max(
            config.cost_floor_bps / 10_000.0,
            config.volatility_multiplier * volatility,
            config.grid_multiplier * grid,
        )
        future_return = math.log(mids[future_index] / mid)
        value = 0 if future_return > threshold else 2 if future_return < -threshold else 1
        labels.append(Label(index, future_index, value, future_return, threshold))
    return labels


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Add causal dynamic-threshold labels to a Rust feature dump."
    )
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--prediction-horizon-ms", type=int, default=60_000)
    parser.add_argument("--volatility-multiplier", type=float, default=0.80)
    parser.add_argument("--grid-multiplier", type=float, default=2.00)
    parser.add_argument("--cost-floor-bps", type=float, default=2.00)
    args = parser.parse_args()

    with args.input.open(newline="", encoding="utf-8") as source:
        rows = list(csv.DictReader(source))
    expected = {"timestamp_ms", "mid", "relative_grid_interval", *FEATURE_NAMES}
    if not rows:
        raise ValueError("feature dump is empty")
    missing = expected.difference(rows[0])
    if missing:
        raise ValueError(f"feature dump is missing columns: {sorted(missing)}")

    timestamps = [int(row["timestamp_ms"]) for row in rows]
    mids = [float(row["mid"]) for row in rows]
    grids = [float(row["relative_grid_interval"]) for row in rows]
    labels = build_labels(
        timestamps,
        mids,
        grids,
        LabelConfig(
            prediction_horizon_ms=args.prediction_horizon_ms,
            volatility_multiplier=args.volatility_multiplier,
            grid_multiplier=args.grid_multiplier,
            cost_floor_bps=args.cost_floor_bps,
        ),
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = ["timestamp", *FEATURE_NAMES, "label"]
    with args.output.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.DictWriter(destination, fieldnames=fieldnames)
        writer.writeheader()
        for label in labels:
            source = rows[label.index]
            writer.writerow(
                {
                    "timestamp": source["timestamp_ms"],
                    **{name: source[name] for name in FEATURE_NAMES},
                    "label": label.value,
                }
            )


if __name__ == "__main__":
    main()
