import math
import unittest

from regime_model.build_labels import LabelConfig, build_labels
from regime_model.calibration import calibrate_temperature, log_loss
from regime_model.evaluate import evaluate
from regime_model.walk_forward import purged_walk_forward


class ValidationTest(unittest.TestCase):
    def test_dynamic_labels_use_historical_volatility_and_future_target(self):
        timestamps = [second * 1_000 for second in range(10)]
        mids = [100.0, 100.0, 100.0, 100.0, 101.0, 102.0, 103.0, 103.0, 103.0, 103.0]
        labels = build_labels(
            timestamps,
            mids,
            [0.0001] * len(mids),
            LabelConfig(prediction_horizon_ms=2_000, volatility_multiplier=0.0, grid_multiplier=1.0, cost_floor_bps=1.0),
        )
        self.assertEqual(labels[2].future_index, 4)
        self.assertEqual(labels[2].value, 0)
        self.assertEqual(labels[-1].future_index, 9)
        self.assertTrue(all(label.threshold >= 0.0001 for label in labels))

    def test_walk_forward_has_purge_and_disjoint_test_windows(self):
        folds = purged_walk_forward(
            100, minimum_train_size=30, validation_size=10, test_size=10, purge_size=5, embargo_size=2
        )
        self.assertGreaterEqual(len(folds), 2)
        for fold in folds:
            self.assertGreaterEqual(fold.validation.start - fold.train.stop, 5)
            self.assertGreaterEqual(fold.test.start - fold.validation.stop, 7)
            self.assertTrue(set(fold.train).isdisjoint(fold.validation))
            self.assertTrue(set(fold.validation).isdisjoint(fold.test))
        self.assertTrue(set(folds[0].test).isdisjoint(folds[1].test))

    def test_temperature_is_selected_only_from_given_validation_rows(self):
        logits = [(8.0, 0.0, -8.0), (-8.0, 0.0, 8.0), (8.0, 0.0, -8.0)]
        labels = [0, 2, 1]
        temperature = calibrate_temperature(logits, labels)
        self.assertGreater(temperature, 1.0)
        self.assertLess(log_loss(logits, labels, temperature), log_loss(logits, labels, 1.0))

    def test_metrics_are_exact_for_perfect_predictions(self):
        metrics = evaluate([0, 1, 2], [(1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)])
        self.assertEqual(metrics.macro_f1, 1.0)
        self.assertEqual(metrics.kappa, 1.0)
        self.assertEqual(metrics.brier_score, 0.0)
        self.assertEqual(metrics.opposite_error_rate, 0.0)
        self.assertTrue(math.isfinite(metrics.log_loss))


if __name__ == "__main__":
    unittest.main()
