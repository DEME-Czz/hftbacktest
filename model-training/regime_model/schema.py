from __future__ import annotations

import hashlib

FEATURE_NAMES = (
    "ret_1s", "ret_5s", "ret_15s", "ret_60s", "ema_spread_10_60", "efficiency_60s",
    "rv_5s", "rv_30s", "rv_300s", "vol_ratio_30_300", "range_60s_bps",
    "spread_bps", "imbalance_l1", "imbalance_l5", "imbalance_l10", "microprice_delta_bps",
    "trade_imbalance_1s", "trade_imbalance_5s", "trade_imbalance_15s", "cvd_slope_15s",
    "trade_intensity_ratio", "depth_l5_log", "depth_change_5s", "ofi_5s",
)

FEATURE_GROUPS = {
    "trend": tuple(range(0, 6)),
    "volatility": tuple(range(6, 11)),
    "book": tuple(range(11, 16)),
    "trades": tuple(range(16, 21)),
    "liquidity": tuple(range(21, 24)),
}


def feature_schema_hash() -> str:
    payload = "\n".join(FEATURE_NAMES).encode()
    return f"sha256:{hashlib.sha256(payload).hexdigest()}"
