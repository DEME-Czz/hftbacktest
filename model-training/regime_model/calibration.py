from __future__ import annotations

import math
from typing import Sequence


def probabilities(logits: Sequence[Sequence[float]], temperature: float) -> list[tuple[float, ...]]:
    if temperature <= 0.0:
        raise ValueError("temperature must be positive")
    result = []
    for row in logits:
        scaled = [value / temperature for value in row]
        maximum = max(scaled)
        exponentials = [math.exp(value - maximum) for value in scaled]
        total = sum(exponentials)
        result.append(tuple(value / total for value in exponentials))
    return result


def log_loss(logits: Sequence[Sequence[float]], labels: Sequence[int], temperature: float) -> float:
    if not logits or len(logits) != len(labels):
        raise ValueError("logits and labels must be non-empty and aligned")
    predicted = probabilities(logits, temperature)
    return -sum(math.log(max(row[label], 1e-300)) for row, label in zip(predicted, labels, strict=True)) / len(labels)


def calibrate_temperature(
    validation_logits: Sequence[Sequence[float]], validation_labels: Sequence[int]
) -> float:
    """Golden-section search on validation NLL; training rows must not be passed here."""
    low, high = math.log(0.05), math.log(20.0)
    ratio = (math.sqrt(5.0) - 1.0) / 2.0
    left = high - ratio * (high - low)
    right = low + ratio * (high - low)
    for _ in range(100):
        if log_loss(validation_logits, validation_labels, math.exp(left)) < log_loss(
            validation_logits, validation_labels, math.exp(right)
        ):
            high, right = right, left
            left = high - ratio * (high - low)
        else:
            low, left = left, right
            right = low + ratio * (high - low)
    return math.exp((low + high) / 2.0)

