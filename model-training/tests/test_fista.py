import math
import unittest

from regime_model.fista import fit_group_lasso, predict
from regime_model.schema import FEATURE_NAMES, feature_schema_hash


class GroupLassoTest(unittest.TestCase):
    def test_learns_three_separable_classes(self):
        rows, labels = [], []
        for value, label in [(-2.0, 2), (-1.5, 2), (-0.1, 1), (0.1, 1), (1.5, 0), (2.0, 0)]:
            row = [0.0] * len(FEATURE_NAMES)
            row[0] = value
            rows.append(row)
            labels.append(label)
        model = fit_group_lasso(rows, labels, 0.001, max_iterations=5_000)
        self.assertEqual(max(range(3), key=predict(model, rows[0]).__getitem__), 2)
        self.assertEqual(max(range(3), key=predict(model, rows[-1]).__getitem__), 0)
        self.assertTrue(all(math.isfinite(value) for value in predict(model, rows[2])))

    def test_schema_hash_is_stable(self):
        self.assertEqual(len(feature_schema_hash()), len("sha256:") + 64)


if __name__ == "__main__":
    unittest.main()
