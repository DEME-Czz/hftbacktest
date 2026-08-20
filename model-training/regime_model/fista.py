from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Sequence

from .schema import FEATURE_GROUPS


@dataclass(frozen=True)
class FitResult:
    intercept_up: float
    intercept_down: float
    coef_up: tuple[float, ...]
    coef_down: tuple[float, ...]
    iterations: int


def _softmax(up: float, down: float) -> tuple[float, float, float]:
    maximum = max(up, down, 0.0)
    values = (math.exp(up - maximum), math.exp(-maximum), math.exp(down - maximum))
    total = sum(values)
    return tuple(value / total for value in values)  # type: ignore[return-value]


def fit_group_lasso(
    features: Sequence[Sequence[float]],
    labels: Sequence[int],
    regularization: float,
    *,
    max_iterations: int = 2_000,
    tolerance: float = 1e-8,
) -> FitResult:
    """Fit UP/SIDEWAYS/DOWN multinomial Logit with group-lasso proximal FISTA.

    Labels are 0=UP, 1=SIDEWAYS, 2=DOWN. The SIDEWAYS logit is the reference class.
    """
    if not features or len(features) != len(labels):
        raise ValueError("features and labels must be non-empty and aligned")
    width = len(features[0])
    if width == 0 or any(len(row) != width for row in features):
        raise ValueError("feature rows must have equal non-zero width")
    if any(label not in (0, 1, 2) for label in labels):
        raise ValueError("labels must be 0, 1, or 2")
    if regularization < 0.0:
        raise ValueError("regularization cannot be negative")

    frequencies = [labels.count(label) for label in range(3)]
    class_weights = [1.0 / math.sqrt(max(frequency, 1)) for frequency in frequencies]
    scale = 3.0 / sum(class_weights)
    class_weights = [min(2.0, max(0.5, weight * scale)) for weight in class_weights]
    weight_sum = sum(class_weights[label] for label in labels)
    max_norm = max(sum(value * value for value in row) for row in features)
    step = 1.0 / max(1.0, 0.5 * max_norm + 0.5)
    weights = [0.0] * (2 * width + 2)
    momentum = weights.copy()
    acceleration = 1.0

    for iteration in range(1, max_iterations + 1):
        gradient = [0.0] * len(weights)
        for row, label in zip(features, labels, strict=True):
            sample_weight = class_weights[label]
            up = momentum[0] + sum(momentum[2 + j] * value for j, value in enumerate(row))
            down = momentum[1] + sum(momentum[2 + width + j] * value for j, value in enumerate(row))
            p_up, _, p_down = _softmax(up, down)
            error_up = p_up - (label == 0)
            error_down = p_down - (label == 2)
            gradient[0] += sample_weight * error_up / weight_sum
            gradient[1] += sample_weight * error_down / weight_sum
            for j, value in enumerate(row):
                gradient[2 + j] += sample_weight * error_up * value / weight_sum
                gradient[2 + width + j] += sample_weight * error_down * value / weight_sum

        candidate = [value - step * grad for value, grad in zip(momentum, gradient, strict=True)]
        for indices in FEATURE_GROUPS.values():
            indices = tuple(index for index in indices if index < width)
            for offset in (2, 2 + width):
                norm = math.sqrt(sum(candidate[offset + index] ** 2 for index in indices))
                shrinkage = max(0.0, 1.0 - step * regularization * math.sqrt(len(indices)) / max(norm, 1e-300))
                for index in indices:
                    candidate[offset + index] *= shrinkage

        delta = max(abs(new - old) for new, old in zip(candidate, weights, strict=True))
        next_acceleration = (1.0 + math.sqrt(1.0 + 4.0 * acceleration * acceleration)) / 2.0
        factor = (acceleration - 1.0) / next_acceleration
        momentum = [new + factor * (new - old) for new, old in zip(candidate, weights, strict=True)]
        weights = candidate
        acceleration = next_acceleration
        if delta < tolerance:
            break

    return FitResult(
        intercept_up=weights[0], intercept_down=weights[1],
        coef_up=tuple(weights[2:2 + width]), coef_down=tuple(weights[2 + width:]),
        iterations=iteration,
    )


def predict(result: FitResult, row: Sequence[float], temperature: float = 1.0) -> tuple[float, float, float]:
    if temperature <= 0.0:
        raise ValueError("temperature must be positive")
    up = (result.intercept_up + sum(a * b for a, b in zip(result.coef_up, row, strict=True))) / temperature
    down = (result.intercept_down + sum(a * b for a, b in zip(result.coef_down, row, strict=True))) / temperature
    return _softmax(up, down)
