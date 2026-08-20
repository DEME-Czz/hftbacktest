from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Fold:
    train: range
    validation: range
    test: range


def purged_walk_forward(
    sample_count: int,
    *,
    minimum_train_size: int,
    validation_size: int,
    test_size: int,
    purge_size: int,
    embargo_size: int = 0,
) -> list[Fold]:
    """Expanding-window folds with gaps around validation and one-shot test segments."""
    values = (sample_count, minimum_train_size, validation_size, test_size, purge_size, embargo_size)
    if any(value < 0 for value in values) or min(minimum_train_size, validation_size, test_size) == 0:
        raise ValueError("sizes must be positive except purge/embargo, which may be zero")
    folds = []
    validation_start = minimum_train_size + purge_size
    while True:
        validation_end = validation_start + validation_size
        test_start = validation_end + purge_size + embargo_size
        test_end = test_start + test_size
        if test_end > sample_count:
            break
        train_end = validation_start - purge_size
        folds.append(
            Fold(
                train=range(0, train_end),
                validation=range(validation_start, validation_end),
                test=range(test_start, test_end),
            )
        )
        validation_start += test_size
    return folds

