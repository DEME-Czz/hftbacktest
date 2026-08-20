from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Sequence


@dataclass(frozen=True)
class Metrics:
    log_loss: float
    macro_f1: float
    kappa: float
    brier_score: float
    opposite_error_rate: float
    expected_calibration_error: float


def evaluate(labels: Sequence[int], probabilities: Sequence[Sequence[float]], bins: int = 10) -> Metrics:
    if not labels or len(labels) != len(probabilities):
        raise ValueError("labels and probabilities must be non-empty and aligned")
    predictions = [max(range(3), key=row.__getitem__) for row in probabilities]
    confusion = [[0] * 3 for _ in range(3)]
    for actual, predicted in zip(labels, predictions, strict=True):
        confusion[actual][predicted] += 1
    f1 = []
    for label in range(3):
        tp = confusion[label][label]
        fp = sum(confusion[actual][label] for actual in range(3) if actual != label)
        fn = sum(confusion[label][predicted] for predicted in range(3) if predicted != label)
        f1.append(2 * tp / max(2 * tp + fp + fn, 1))
    count = len(labels)
    observed = sum(confusion[i][i] for i in range(3)) / count
    expected = sum(
        sum(confusion[i]) * sum(confusion[row][i] for row in range(3)) for i in range(3)
    ) / (count * count)
    kappa = (observed - expected) / max(1.0 - expected, 1e-300)
    brier = sum(
        sum((row[label] - (actual == label)) ** 2 for label in range(3))
        for actual, row in zip(labels, probabilities, strict=True)
    ) / count
    opposite = sum(
        (actual == 0 and predicted == 2) or (actual == 2 and predicted == 0)
        for actual, predicted in zip(labels, predictions, strict=True)
    ) / count
    losses = -sum(
        math.log(max(row[actual], 1e-300))
        for actual, row in zip(labels, probabilities, strict=True)
    ) / count
    calibration = 0.0
    confidences = [max(row) for row in probabilities]
    for bin_index in range(bins):
        members = [i for i, confidence in enumerate(confidences) if bin_index / bins <= confidence < (bin_index + 1) / bins or (bin_index == bins - 1 and confidence == 1.0)]
        if members:
            accuracy = sum(predictions[i] == labels[i] for i in members) / len(members)
            confidence = sum(confidences[i] for i in members) / len(members)
            calibration += len(members) / count * abs(accuracy - confidence)
    return Metrics(losses, sum(f1) / 3.0, kappa, brier, opposite, calibration)
