use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Debug,
    fs,
    path::Path,
};

use hftbacktest::prelude::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const FEATURE_NAMES: [&str; 24] = [
    "ret_1s",
    "ret_5s",
    "ret_15s",
    "ret_60s",
    "ema_spread_10_60",
    "efficiency_60s",
    "rv_5s",
    "rv_30s",
    "rv_300s",
    "vol_ratio_30_300",
    "range_60s_bps",
    "spread_bps",
    "imbalance_l1",
    "imbalance_l5",
    "imbalance_l10",
    "microprice_delta_bps",
    "trade_imbalance_1s",
    "trade_imbalance_5s",
    "trade_imbalance_15s",
    "cvd_slope_15s",
    "trade_intensity_ratio",
    "depth_l5_log",
    "depth_change_5s",
    "ofi_5s",
];

#[derive(Debug)]
pub struct MultinomialModel {
    pub version: String,
    pub prediction_horizon_ms: i64,
    mean: Vec<f64>,
    std: Vec<f64>,
    intercept_up: f64,
    intercept_down: f64,
    coef_up: Vec<f64>,
    coef_down: Vec<f64>,
    temperature: f64,
}

impl MultinomialModel {
    pub fn load(path: impl AsRef<Path>, expected_horizon_ms: i64) -> Result<Self, String> {
        let json = fs::read_to_string(path.as_ref())
            .map_err(|error| format!("cannot read model {}: {error}", path.as_ref().display()))?;
        Self::from_json(&json, expected_horizon_ms)
    }

    fn from_json(json: &str, expected_horizon_ms: i64) -> Result<Self, String> {
        let value: Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let string = |name: &str| {
            value[name]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("missing or invalid {name}"))
        };
        let number = |name: &str| {
            value[name]
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| format!("missing or invalid {name}"))
        };
        let vector = |name: &str| -> Result<Vec<f64>, String> {
            value[name]
                .as_array()
                .ok_or_else(|| format!("missing or invalid {name}"))?
                .iter()
                .map(|item| {
                    item.as_f64()
                        .filter(|number| number.is_finite())
                        .ok_or_else(|| format!("{name} contains a non-finite value"))
                })
                .collect()
        };
        if string("model_type")? != "multinomial_group_lasso" {
            return Err("model_type must be multinomial_group_lasso".to_owned());
        }
        let version = string("version")?;
        if version.trim().is_empty() {
            return Err("model version cannot be empty".to_owned());
        }
        let horizon = value["prediction_horizon_ms"]
            .as_i64()
            .ok_or_else(|| "missing or invalid prediction_horizon_ms".to_owned())?;
        if horizon != expected_horizon_ms {
            return Err(format!(
                "prediction horizon mismatch: model={horizon}, configured={expected_horizon_ms}"
            ));
        }
        let features = value["features"]
            .as_array()
            .ok_or_else(|| "missing or invalid features".to_owned())?;
        if features.len() != FEATURE_NAMES.len()
            || features
                .iter()
                .zip(FEATURE_NAMES)
                .any(|(actual, expected)| actual.as_str() != Some(expected))
        {
            return Err("model feature order does not match the Rust schema".to_owned());
        }
        let expected_hash = feature_schema_hash();
        if string("feature_schema_hash")? != expected_hash {
            return Err("feature_schema_hash mismatch".to_owned());
        }
        let mean = vector("mean")?;
        let std = vector("std")?;
        let coef_up = vector("coef_up")?;
        let coef_down = vector("coef_down")?;
        for (name, values) in [
            ("mean", &mean),
            ("std", &std),
            ("coef_up", &coef_up),
            ("coef_down", &coef_down),
        ] {
            if values.len() != FEATURE_NAMES.len() {
                return Err(format!(
                    "{name} must contain {} values",
                    FEATURE_NAMES.len()
                ));
            }
        }
        if std.iter().any(|value| *value <= 0.0) {
            return Err("standard deviations must be positive".to_owned());
        }
        if coef_up.iter().all(|value| *value == 0.0) && coef_down.iter().all(|value| *value == 0.0)
        {
            return Err("refusing an all-zero model".to_owned());
        }
        let temperature = number("temperature")?;
        if temperature <= 0.0 {
            return Err("temperature must be positive".to_owned());
        }
        Ok(Self {
            version,
            prediction_horizon_ms: horizon,
            mean,
            std,
            intercept_up: number("intercept_up")?,
            intercept_down: number("intercept_down")?,
            coef_up,
            coef_down,
            temperature,
        })
    }

    pub fn predict(&self, features: &[f64]) -> Result<Prediction, String> {
        if features.len() != FEATURE_NAMES.len() || features.iter().any(|value| !value.is_finite())
        {
            return Err("invalid feature vector".to_owned());
        }
        let standardized = features
            .iter()
            .zip(&self.mean)
            .zip(&self.std)
            .map(|((&value, &mean), &std)| ((value - mean) / std).clamp(-8.0, 8.0));
        let (mut up, mut down) = (self.intercept_up, self.intercept_down);
        for ((value, up_coef), down_coef) in standardized.zip(&self.coef_up).zip(&self.coef_down) {
            up += value * up_coef;
            down += value * down_coef;
        }
        up /= self.temperature;
        down /= self.temperature;
        let max_logit = up.max(down).max(0.0);
        let up = (up - max_logit).exp();
        let sideways = (-max_logit).exp();
        let down = (down - max_logit).exp();
        let sum = up + sideways + down;
        let prediction = Prediction {
            up: up / sum,
            sideways: sideways / sum,
            down: down / sum,
        };
        prediction
            .valid()
            .then_some(prediction)
            .ok_or_else(|| "model produced invalid probabilities".to_owned())
    }
}

pub fn feature_schema_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(FEATURE_NAMES.join("\n"));
    format!("sha256:{:x}", hasher.finalize())
}

#[derive(Clone, Copy, Debug)]
struct MarketSample {
    timestamp: i64,
    mid: f64,
    spread_bps: f64,
    bid_qty: [f64; 3],
    ask_qty: [f64; 3],
    microprice_delta_bps: f64,
}

#[derive(Clone, Copy, Debug)]
struct TradeSample {
    timestamp: i64,
    signed_qty: f64,
}

/// Causal feature implementation shared by rule/model inference in the Rust runtime.
pub struct FeatureEngine {
    market: VecDeque<MarketSample>,
    trades: VecDeque<TradeSample>,
    sample_interval_ns: i64,
    last_sample_at: Option<i64>,
}

impl FeatureEngine {
    pub fn new(sample_interval_ns: i64) -> Self {
        Self {
            market: VecDeque::new(),
            trades: VecDeque::new(),
            sample_interval_ns,
            last_sample_at: None,
        }
    }

    pub fn on_trades(&mut self, trades: &[Event], now: i64) {
        for trade in trades.iter().filter(|trade| trade.is(TRADE_EVENT)) {
            let signed_qty = if trade.is(BUY_EVENT) {
                trade.qty
            } else if trade.is(SELL_EVENT) {
                -trade.qty
            } else {
                continue;
            };
            self.trades.push_back(TradeSample {
                timestamp: trade.local_ts.min(now),
                signed_qty,
            });
        }
        while self
            .trades
            .front()
            .is_some_and(|trade| now - trade.timestamp > 300_000_000_000)
        {
            self.trades.pop_front();
        }
    }

    pub fn sample<MD: MarketDepth>(&mut self, depth: &MD, now: i64) -> bool {
        if self
            .last_sample_at
            .is_some_and(|last| now - last < self.sample_interval_ns)
        {
            return false;
        }
        let bid = depth.best_bid();
        let ask = depth.best_ask();
        let mid = (bid + ask) / 2.0;
        if !mid.is_finite() || mid <= 0.0 || bid >= ask {
            return false;
        }
        let bid_tick = depth.best_bid_tick();
        let ask_tick = depth.best_ask_tick();
        let sums = |levels: i64, is_bid: bool| {
            (0..levels)
                .map(|offset| {
                    if is_bid {
                        depth.bid_qty_at_tick(bid_tick - offset)
                    } else {
                        depth.ask_qty_at_tick(ask_tick + offset)
                    }
                })
                .sum()
        };
        let bid_l1 = depth.best_bid_qty();
        let ask_l1 = depth.best_ask_qty();
        let microprice = (ask * bid_l1 + bid * ask_l1) / (bid_l1 + ask_l1).max(f64::EPSILON);
        self.market.push_back(MarketSample {
            timestamp: now,
            mid,
            spread_bps: (ask - bid) / mid * 10_000.0,
            bid_qty: [bid_l1, sums(5, true), sums(10, true)],
            ask_qty: [ask_l1, sums(5, false), sums(10, false)],
            microprice_delta_bps: (microprice - mid) / mid * 10_000.0,
        });
        self.last_sample_at = Some(now);
        while self
            .market
            .front()
            .is_some_and(|sample| now - sample.timestamp > 305_000_000_000)
        {
            self.market.pop_front();
        }
        true
    }

    pub fn snapshot(&self) -> Option<[f64; FEATURE_NAMES.len()]> {
        let latest = *self.market.back()?;
        if latest.timestamp - self.market.front()?.timestamp < 300_000_000_000 {
            return None;
        }
        let ret = |seconds: i64| {
            let old = self.market_before(latest.timestamp - seconds * 1_000_000_000)?;
            Some((latest.mid / old.mid).ln())
        };
        let rv = |seconds: i64| {
            let start = latest.timestamp - seconds * 1_000_000_000;
            let mut previous: Option<f64> = None;
            let mut sum: f64 = 0.0;
            for sample in self
                .market
                .iter()
                .filter(|sample| sample.timestamp >= start)
            {
                if let Some(old_mid) = previous {
                    let change: f64 = (sample.mid / old_mid).ln();
                    sum += change * change;
                }
                previous = Some(sample.mid);
            }
            sum.sqrt()
        };
        let weighted_mid = |half_life_seconds: f64| {
            let decay = std::f64::consts::LN_2 / half_life_seconds;
            let (weighted, weights) = self.market.iter().fold((0.0, 0.0), |acc, sample| {
                let age = (latest.timestamp - sample.timestamp) as f64 / 1e9;
                let weight = (-decay * age).exp();
                (acc.0 + sample.mid * weight, acc.1 + weight)
            });
            weighted / weights
        };
        let samples_60: Vec<_> = self
            .market
            .iter()
            .filter(|sample| latest.timestamp - sample.timestamp <= 60_000_000_000)
            .collect();
        let path = samples_60
            .windows(2)
            .map(|pair| (pair[1].mid - pair[0].mid).abs())
            .sum::<f64>();
        let efficiency = (latest.mid - samples_60.first()?.mid).abs() / path.max(f64::EPSILON);
        let (low, high) = samples_60
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), sample| {
                (low.min(sample.mid), high.max(sample.mid))
            });
        let imbalance = |level: usize| {
            (latest.bid_qty[level] - latest.ask_qty[level])
                / (latest.bid_qty[level] + latest.ask_qty[level]).max(f64::EPSILON)
        };
        let trade_stats = |seconds: i64| {
            let start = latest.timestamp - seconds * 1_000_000_000;
            self.trades
                .iter()
                .filter(|trade| trade.timestamp >= start)
                .fold((0.0_f64, 0.0_f64, 0_usize), |(buy, sell, count), trade| {
                    (
                        buy + trade.signed_qty.max(0.0),
                        sell + (-trade.signed_qty).max(0.0),
                        count + 1,
                    )
                })
        };
        let trade_imbalance = |seconds| {
            let (buy, sell, _) = trade_stats(seconds);
            (buy - sell) / (buy + sell).max(f64::EPSILON)
        };
        let (_, _, count_5) = trade_stats(5);
        let (_, _, count_60) = trade_stats(60);
        let (buy_15, sell_15, _) = trade_stats(15);
        let old_5 = self.market_before(latest.timestamp - 5_000_000_000)?;
        let depth_l5 = latest.bid_qty[1] + latest.ask_qty[1];
        let old_depth_l5 = old_5.bid_qty[1] + old_5.ask_qty[1];
        let bid_change = latest.bid_qty[1] - old_5.bid_qty[1];
        let ask_change = latest.ask_qty[1] - old_5.ask_qty[1];
        let rv_30 = rv(30);
        let rv_300 = rv(300);
        Some([
            ret(1)?,
            ret(5)?,
            ret(15)?,
            ret(60)?,
            (weighted_mid(10.0) - weighted_mid(60.0)) / latest.mid,
            efficiency,
            rv(5),
            rv_30,
            rv_300,
            rv_30 / rv_300.max(f64::EPSILON),
            (high - low) / latest.mid * 10_000.0,
            latest.spread_bps,
            imbalance(0),
            imbalance(1),
            imbalance(2),
            latest.microprice_delta_bps,
            trade_imbalance(1),
            trade_imbalance(5),
            trade_imbalance(15),
            (buy_15 - sell_15) / 15.0,
            count_5 as f64 / (count_60 as f64 / 12.0).max(1.0),
            depth_l5.ln_1p(),
            (depth_l5 - old_depth_l5) / old_depth_l5.max(f64::EPSILON),
            (bid_change - ask_change) / (bid_change.abs() + ask_change.abs()).max(f64::EPSILON),
        ])
    }

    fn market_before(&self, timestamp: i64) -> Option<&MarketSample> {
        self.market
            .iter()
            .rev()
            .find(|sample| sample.timestamp <= timestamp)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    Up,
    Sideways,
    Down,
    Uncertain,
    RiskOff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskOffReason {
    InvalidModel,
    InvalidPrediction,
    PredictionStale,
    MarketDataIncomplete,
    MarketDataStale,
    CrossedBook,
    SpreadLimit,
    PositionLimit,
    UnknownOrder,
    DailyLossLimit,
    DrawdownLimit,
    OrderExposureMismatch,
    VolatilityShock,
}

#[derive(Clone, Copy, Debug)]
pub struct Prediction {
    pub up: f64,
    pub sideways: f64,
    pub down: f64,
}

impl Prediction {
    pub fn valid(self) -> bool {
        let sum = self.up + self.sideways + self.down;
        self.up.is_finite()
            && self.sideways.is_finite()
            && self.down.is_finite()
            && self.up >= 0.0
            && self.sideways >= 0.0
            && self.down >= 0.0
            && (sum - 1.0).abs() < 1e-6
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RegimeConfig {
    pub trend_enter_probability: f64,
    pub sideways_enter_probability: f64,
    pub trend_margin: f64,
    pub sideways_margin: f64,
    pub exit_probability: f64,
    pub confirmation_count: usize,
    pub minimum_hold_ns: i64,
    pub stale_after_ns: i64,
}

impl Default for RegimeConfig {
    fn default() -> Self {
        Self {
            trend_enter_probability: 0.62,
            sideways_enter_probability: 0.58,
            trend_margin: 0.15,
            sideways_margin: 0.10,
            exit_probability: 0.50,
            confirmation_count: 3,
            minimum_hold_ns: 5_000_000_000,
            stale_after_ns: 3_000_000_000,
        }
    }
}

pub struct RegimeMachine {
    config: RegimeConfig,
    current: Regime,
    candidate: Option<Regime>,
    confirmations: usize,
    changed_at: i64,
    last_prediction_at: Option<i64>,
    last_prediction: Option<Prediction>,
    risk_off_reason: Option<RiskOffReason>,
    version: u64,
}

impl RegimeMachine {
    pub fn new(config: RegimeConfig) -> Self {
        Self {
            config,
            current: Regime::Uncertain,
            candidate: None,
            confirmations: 0,
            changed_at: i64::MIN,
            last_prediction_at: None,
            last_prediction: None,
            risk_off_reason: None,
            version: 0,
        }
    }

    pub fn current(&self) -> Regime {
        self.current
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn has_prediction(&self) -> bool {
        self.last_prediction_at.is_some()
    }

    pub fn last_prediction(&self) -> Option<Prediction> {
        self.last_prediction
    }

    pub fn risk_off_reason(&self) -> Option<RiskOffReason> {
        self.risk_off_reason
    }

    pub fn update(&mut self, prediction: Prediction, now: i64) -> Regime {
        if !prediction.valid() {
            return self.risk_off_with(RiskOffReason::InvalidPrediction, now);
        }
        self.last_prediction_at = Some(now);
        self.last_prediction = Some(prediction);

        let current_probability = match self.current {
            Regime::Up => prediction.up,
            Regime::Sideways => prediction.sideways,
            Regime::Down => prediction.down,
            Regime::Uncertain | Regime::RiskOff => 1.0,
        };
        if matches!(self.current, Regime::Up | Regime::Sideways | Regime::Down)
            && current_probability < self.config.exit_probability
        {
            self.transition(Regime::Uncertain, now);
            return self.current;
        }

        let proposed = classify(prediction, self.config);
        let proposed = match (self.current, proposed) {
            (Regime::Up, Regime::Down) | (Regime::Down, Regime::Up) => Regime::Uncertain,
            (_, proposed) => proposed,
        };
        if proposed == self.current {
            self.candidate = None;
            self.confirmations = 0;
            return self.current;
        }
        if now.saturating_sub(self.changed_at) < self.config.minimum_hold_ns
            && self.current != Regime::RiskOff
        {
            return self.current;
        }
        if proposed == Regime::Uncertain {
            self.transition(proposed, now);
        } else if self.candidate == Some(proposed) {
            self.confirmations += 1;
            if self.confirmations >= self.config.confirmation_count {
                self.transition(proposed, now);
            }
        } else {
            self.candidate = Some(proposed);
            self.confirmations = 1;
        }
        self.current
    }

    pub fn check_stale(&mut self, now: i64) -> Regime {
        if self
            .last_prediction_at
            .is_none_or(|last| now - last > self.config.stale_after_ns)
        {
            self.risk_off_with(RiskOffReason::PredictionStale, now);
        }
        self.current
    }

    pub fn risk_off(&mut self, now: i64) -> Regime {
        self.risk_off_with(RiskOffReason::InvalidPrediction, now)
    }

    pub fn risk_off_with(&mut self, reason: RiskOffReason, now: i64) -> Regime {
        self.risk_off_reason = Some(reason);
        self.transition(Regime::RiskOff, now);
        self.current
    }

    fn transition(&mut self, next: Regime, now: i64) {
        if next != Regime::RiskOff {
            self.risk_off_reason = None;
        }
        if self.current != next {
            self.current = next;
            self.changed_at = now;
            self.version += 1;
        }
        self.candidate = None;
        self.confirmations = 0;
    }
}

fn classify(p: Prediction, config: RegimeConfig) -> Regime {
    if p.up >= config.trend_enter_probability
        && p.up - p.sideways.max(p.down) >= config.trend_margin
    {
        Regime::Up
    } else if p.down >= config.trend_enter_probability
        && p.down - p.sideways.max(p.up) >= config.trend_margin
    {
        Regime::Down
    } else if p.sideways >= config.sideways_enter_probability
        && p.sideways - p.up.max(p.down) >= config.sideways_margin
    {
        Regime::Sideways
    } else {
        Regime::Uncertain
    }
}

/// A causal, lightweight three-class signal for the runnable example. Production can replace this
/// with model probabilities without changing the state machine or order policy.
pub struct ReturnClassifier {
    samples: VecDeque<(i64, f64)>,
    horizon_ns: i64,
    trend_threshold: f64,
}

impl ReturnClassifier {
    pub fn new(horizon_ns: i64, trend_threshold: f64) -> Self {
        Self {
            samples: VecDeque::new(),
            horizon_ns,
            trend_threshold,
        }
    }

    pub fn update(&mut self, now: i64, mid: f64) -> Option<Prediction> {
        self.samples.push_back((now, mid));
        while self.samples.len() > 2 && now - self.samples[1].0 >= self.horizon_ns {
            self.samples.pop_front();
        }
        let &(then, old_mid) = self.samples.front()?;
        if now - then < self.horizon_ns || old_mid <= 0.0 || mid <= 0.0 {
            return None;
        }
        let ret = (mid / old_mid).ln();
        let score = (ret / self.trend_threshold.max(f64::EPSILON)).clamp(-2.0, 2.0);
        let up_logit = score;
        let down_logit = -score;
        let sideways_logit = 1.1 - score.abs();
        let max_logit = up_logit.max(down_logit).max(sideways_logit);
        let up = (up_logit - max_logit).exp();
        let sideways = (sideways_logit - max_logit).exp();
        let down = (down_logit - max_logit).exp();
        let sum = up + sideways + down;
        Some(Prediction {
            up: up / sum,
            sideways: sideways / sum,
            down: down / sum,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OrderIntent {
    OpenLong,
    ReduceShort,
    OpenShort,
    ReduceLong,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrategyOrderMeta {
    pub intent: OrderIntent,
    pub created_regime: Regime,
    pub regime_version: u64,
    pub price_tick: i64,
    pub qty: f64,
}

#[derive(Debug, Default)]
pub struct OrderRegistry {
    orders: HashMap<u64, StrategyOrderMeta>,
}

impl OrderRegistry {
    pub fn recover(&mut self, orders: &HashMap<u64, Order>) -> Result<(), u64> {
        for order in orders
            .values()
            .filter(|order| order.active() || order.pending())
        {
            let (intent, price_tick) = decode_order_id(order.order_id).ok_or(order.order_id)?;
            self.orders
                .entry(order.order_id)
                .or_insert(StrategyOrderMeta {
                    intent,
                    created_regime: Regime::Uncertain,
                    regime_version: 0,
                    price_tick,
                    qty: order.leaves_qty,
                });
        }
        Ok(())
    }

    pub fn sync(&mut self, orders: &HashMap<u64, Order>, regime: Regime, regime_version: u64) {
        self.orders.retain(|id, _| {
            orders
                .get(id)
                .is_some_and(|order| order.active() || order.pending())
        });
        for order in orders
            .values()
            .filter(|order| order.active() || order.pending())
        {
            if let Some((intent, price_tick)) = decode_order_id(order.order_id) {
                self.orders
                    .entry(order.order_id)
                    .or_insert(StrategyOrderMeta {
                        intent,
                        created_regime: regime,
                        regime_version,
                        price_tick,
                        qty: order.leaves_qty,
                    });
            }
        }
    }

    pub fn get(&self, order_id: u64) -> Option<&StrategyOrderMeta> {
        self.orders.get(&order_id)
    }
}

impl OrderIntent {
    pub fn is_open(self) -> bool {
        matches!(self, Self::OpenLong | Self::OpenShort)
    }
}

#[derive(Debug, Default)]
pub struct StrategyMetrics {
    pub direction_violations: u64,
    pub countertrend_open_qty: f64,
    pub long_to_short_reversals: u64,
    pub short_to_long_reversals: u64,
    pub risk_off_count: u64,
    previous_position: Option<f64>,
    previous_regime: Option<Regime>,
    observed_fills: HashSet<(u64, i64)>,
}

impl StrategyMetrics {
    pub fn observe(&mut self, regime: Regime, position: f64, orders: &HashMap<u64, Order>) {
        if self.previous_regime != Some(Regime::RiskOff) && regime == Regime::RiskOff {
            self.risk_off_count += 1;
        }
        if let Some(previous) = self.previous_position {
            if previous > 0.0 && position < 0.0 {
                self.long_to_short_reversals += 1;
            } else if previous < 0.0 && position > 0.0 {
                self.short_to_long_reversals += 1;
            }
        }
        for order in orders.values().filter(|order| order.exec_qty > 0.0) {
            if !self
                .observed_fills
                .insert((order.order_id, order.exch_timestamp))
            {
                continue;
            }
            let Some((intent, _)) = decode_order_id(order.order_id) else {
                self.direction_violations += 1;
                continue;
            };
            let violates = matches!(
                (regime, intent),
                (Regime::Up, OrderIntent::OpenShort)
                    | (Regime::Down, OrderIntent::OpenLong)
                    | (
                        Regime::Uncertain | Regime::RiskOff,
                        OrderIntent::OpenLong | OrderIntent::OpenShort
                    )
            );
            if violates {
                self.direction_violations += 1;
                self.countertrend_open_qty += order.exec_qty;
            }
        }
        self.previous_position = Some(position);
        self.previous_regime = Some(regime);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectionPolicy {
    pub open_long: bool,
    pub open_short: bool,
    pub reduce_long: bool,
    pub reduce_short: bool,
}

impl DirectionPolicy {
    pub fn resolve(regime: Regime, position: f64) -> Self {
        let (mut open_long, mut open_short) = match regime {
            Regime::Up => (true, false),
            Regime::Sideways => (true, true),
            Regime::Down => (false, true),
            Regime::Uncertain | Regime::RiskOff => (false, false),
        };
        // Never allow an opening order to cross an existing position through zero.
        if position > 0.0 {
            open_short = false;
        } else if position < 0.0 {
            open_long = false;
        }
        Self {
            open_long,
            open_short,
            reduce_long: position > 0.0,
            reduce_short: position < 0.0,
        }
    }

    pub fn allows(self, intent: OrderIntent) -> bool {
        match intent {
            OrderIntent::OpenLong => self.open_long,
            OrderIntent::ReduceShort => self.reduce_short,
            OrderIntent::OpenShort => self.open_short,
            OrderIntent::ReduceLong => self.reduce_long,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Budgets {
    pub open_long: f64,
    pub reduce_short: f64,
    pub open_short: f64,
    pub reduce_long: f64,
}

impl Budgets {
    pub fn calculate(
        position: f64,
        max_long: f64,
        max_short: f64,
        pending: &HashMap<OrderIntent, f64>,
    ) -> Self {
        let long = position.max(0.0);
        let short = (-position).max(0.0);
        let get = |intent| pending.get(&intent).copied().unwrap_or(0.0);
        Self {
            open_long: (max_long - long - get(OrderIntent::OpenLong)).max(0.0),
            reduce_short: (short - get(OrderIntent::ReduceShort)).max(0.0),
            open_short: (max_short - short - get(OrderIntent::OpenShort)).max(0.0),
            reduce_long: (long - get(OrderIntent::ReduceLong)).max(0.0),
        }
    }

    pub fn for_intent(self, intent: OrderIntent) -> f64 {
        match intent {
            OrderIntent::OpenLong => self.open_long,
            OrderIntent::ReduceShort => self.reduce_short,
            OrderIntent::OpenShort => self.open_short,
            OrderIntent::ReduceLong => self.reduce_long,
        }
    }
}

fn has_forbidden_pending(orders: &HashMap<u64, Order>, policy: DirectionPolicy) -> bool {
    orders
        .values()
        .filter(|order| order.active() || order.pending())
        .any(|order| {
            decode_order_id(order.order_id).is_none_or(|(intent, _)| !policy.allows(intent))
        })
}

fn order_exposure_consistent(position: f64, orders: &HashMap<u64, Order>) -> bool {
    let mut reduce_long = 0.0;
    let mut reduce_short = 0.0;
    for order in orders
        .values()
        .filter(|order| order.active() || order.pending())
    {
        match decode_order_id(order.order_id) {
            Some((OrderIntent::ReduceLong, _)) => reduce_long += order.leaves_qty,
            Some((OrderIntent::ReduceShort, _)) => reduce_short += order.leaves_qty,
            Some(_) => {}
            None => return false,
        }
    }
    reduce_long <= position.max(0.0) + 1e-12 && reduce_short <= (-position).max(0.0) + 1e-12
}

const INTENT_SHIFT: u32 = 61;
const PRICE_MASK: u64 = (1_u64 << INTENT_SHIFT) - 1;

pub fn encode_order_id(intent: OrderIntent, price_tick: i64) -> Option<u64> {
    let tag = match intent {
        OrderIntent::OpenLong => 0,
        OrderIntent::ReduceShort => 1,
        OrderIntent::OpenShort => 2,
        OrderIntent::ReduceLong => 3,
    };
    let price_tick = u64::try_from(price_tick).ok()?;
    (price_tick <= PRICE_MASK).then_some((tag << INTENT_SHIFT) | price_tick)
}

pub fn decode_order_id(order_id: u64) -> Option<(OrderIntent, i64)> {
    let intent = match order_id >> INTENT_SHIFT {
        0 => OrderIntent::OpenLong,
        1 => OrderIntent::ReduceShort,
        2 => OrderIntent::OpenShort,
        3 => OrderIntent::ReduceLong,
        _ => return None,
    };
    Some((intent, (order_id & PRICE_MASK) as i64))
}

#[derive(Clone, Debug)]
pub struct GridConfig {
    pub relative_half_spread: f64,
    pub relative_grid_interval: f64,
    pub min_grid_step: f64,
    pub sideways_levels: usize,
    pub trend_levels: usize,
    pub reduce_levels: usize,
    pub order_qty: f64,
    pub max_long: f64,
    pub max_short: f64,
    pub return_horizon_ns: i64,
    pub trend_return_threshold: f64,
    pub model_path: Option<String>,
    pub prediction_horizon_ms: i64,
    pub max_spread_bps: f64,
    pub max_position_hard: f64,
    pub market_data_stale_ns: i64,
    pub max_daily_loss: f64,
    pub max_drawdown: f64,
    pub edge_full: f64,
    pub min_directional_limit_ratio: f64,
    pub alpha_multiplier: f64,
    pub alpha_max_grid_intervals: f64,
    pub inventory_skew: f64,
    pub reduce_spread_factor: f64,
    pub volatility_shock_multiple: f64,
}

fn volatility_shock(features: Option<[f64; FEATURE_NAMES.len()]>, multiple: f64) -> bool {
    let Some(features) = features else {
        return false;
    };
    let expected_5s = features[8] * (5.0_f64 / 300.0).sqrt();
    features[6] > expected_5s * multiple.max(0.0) && features[6] > 0.0
}

fn target_position(
    prediction: Option<Prediction>,
    max_long: f64,
    max_short: f64,
    edge_full: f64,
) -> f64 {
    let Some(prediction) = prediction else {
        return 0.0;
    };
    let edge = prediction.up - prediction.down;
    if edge >= 0.0 {
        max_long * (edge / edge_full.max(f64::EPSILON)).clamp(0.0, 1.0)
    } else {
        max_short * (edge / edge_full.max(f64::EPSILON)).clamp(-1.0, 0.0)
    }
}

fn directional_limit(
    regime: Regime,
    prediction: Option<Prediction>,
    maximum: f64,
    minimum_ratio: f64,
    enter_probability: f64,
) -> f64 {
    if !matches!(regime, Regime::Up | Regime::Down) {
        return maximum;
    }
    let confidence = prediction
        .map(|p| {
            (p.up.max(p.sideways).max(p.down) - enter_probability)
                / (1.0 - enter_probability).max(f64::EPSILON)
        })
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    maximum * (minimum_ratio.clamp(0.0, 1.0) + (1.0 - minimum_ratio.clamp(0.0, 1.0)) * confidence)
}

fn forecast_mid_price(
    mid: f64,
    prediction: Option<Prediction>,
    horizon_volatility: f64,
    alpha_multiplier: f64,
    grid_interval: f64,
    max_grid_intervals: f64,
) -> f64 {
    let edge = prediction.map(|p| p.up - p.down).unwrap_or(0.0);
    let maximum_log_offset = grid_interval / mid * max_grid_intervals.max(0.0);
    let alpha = (alpha_multiplier * edge * horizon_volatility)
        .clamp(-maximum_log_offset, maximum_log_offset);
    mid * alpha.exp()
}

pub fn regime_gridtrading<MD, I, R>(
    hbt: &mut I,
    recorder: &mut R,
    config: GridConfig,
) -> Result<(), i64>
where
    MD: MarketDepth,
    I: Bot<MD>,
    I::Error: Debug,
    R: Recorder,
    R::Error: Debug,
{
    if config.order_qty <= 0.0
        || config.max_long <= 0.0
        || config.max_short <= 0.0
        || config.edge_full <= 0.0
        || config.max_position_hard <= 0.0
    {
        return Err(-1);
    }
    let tick_size = hbt.depth(0).tick_size() as f64;
    let min_grid_step = (config.min_grid_step / tick_size).round().max(1.0) * tick_size;
    let mut classifier =
        ReturnClassifier::new(config.return_horizon_ns, config.trend_return_threshold);
    let model = config
        .model_path
        .as_ref()
        .map(|path| MultinomialModel::load(path, config.prediction_horizon_ms));
    let model_load_failed = model.as_ref().is_some_and(Result::is_err);
    let model = model.and_then(Result::ok);
    let mut features = FeatureEngine::new(1_000_000_000);
    let mut latest_features: Option<[f64; FEATURE_NAMES.len()]> = None;
    let mut machine = RegimeMachine::new(RegimeConfig::default());
    let mut metrics = StrategyMetrics::default();
    let mut order_registry = OrderRegistry::default();
    let _ = order_registry.recover(hbt.orders(0));
    let starting_balance = hbt.state_values(0).balance;
    let mut peak_balance = starting_balance;
    let mut iteration = 0_u64;

    while hbt.elapse(100_000_000).unwrap() == ElapseResult::Ok {
        iteration += 1;
        if iteration % 10 == 0 {
            recorder.record(hbt).unwrap();
        }
        let now = hbt.current_timestamp();
        let balance = hbt.state_values(0).balance;
        peak_balance = peak_balance.max(balance);
        let unknown_order = hbt.orders(0).values().any(|order| {
            (order.active() || order.pending()) && decode_order_id(order.order_id).is_none()
        });
        let trades = hbt.last_trades(0).to_vec();
        features.on_trades(&trades, now);
        hbt.clear_last_trades(Some(0));
        let (best_bid_tick, best_ask_tick, best_bid, best_ask, depth_timestamp) = {
            let depth = hbt.depth(0);
            (
                depth.best_bid_tick(),
                depth.best_ask_tick(),
                depth.best_bid() as f64,
                depth.best_ask() as f64,
                depth.timestamp(),
            )
        };
        if best_bid_tick == INVALID_MIN || best_ask_tick == INVALID_MAX {
            machine.risk_off_with(RiskOffReason::MarketDataIncomplete, now);
            cancel_opening_orders(hbt);
            continue;
        }
        if best_bid_tick >= best_ask_tick {
            machine.risk_off_with(RiskOffReason::CrossedBook, now);
            cancel_opening_orders(hbt);
            continue;
        }
        let mid = (best_bid + best_ask) / 2.0;
        let spread_bps = (best_ask - best_bid) / mid * 10_000.0;
        let sampled = features.sample(hbt.depth(0), now);
        let risk_reason = if model_load_failed {
            Some(RiskOffReason::InvalidModel)
        } else if unknown_order {
            Some(RiskOffReason::UnknownOrder)
        } else if depth_timestamp
            .is_some_and(|timestamp| now - timestamp > config.market_data_stale_ns)
        {
            Some(RiskOffReason::MarketDataStale)
        } else if spread_bps > config.max_spread_bps {
            Some(RiskOffReason::SpreadLimit)
        } else if volatility_shock(latest_features, config.volatility_shock_multiple) {
            Some(RiskOffReason::VolatilityShock)
        } else if hbt.position(0).abs() > config.max_position_hard {
            Some(RiskOffReason::PositionLimit)
        } else if !order_exposure_consistent(hbt.position(0), hbt.orders(0)) {
            Some(RiskOffReason::OrderExposureMismatch)
        } else if balance - starting_balance <= -config.max_daily_loss {
            Some(RiskOffReason::DailyLossLimit)
        } else if peak_balance - balance >= config.max_drawdown {
            Some(RiskOffReason::DrawdownLimit)
        } else {
            None
        };
        if let Some(reason) = risk_reason {
            machine.risk_off_with(reason, now);
        } else if sampled {
            let snapshot = features.snapshot();
            if snapshot.is_some() {
                latest_features = snapshot;
            }
            let prediction = if let Some(model) = model.as_ref() {
                snapshot.map(|snapshot| model.predict(&snapshot))
            } else {
                classifier.update(now, mid).map(Ok)
            };
            if let Some(prediction) = prediction {
                match prediction {
                    Ok(prediction) => {
                        machine.update(prediction, now);
                    }
                    Err(_) => {
                        machine.risk_off_with(RiskOffReason::InvalidPrediction, now);
                    }
                }
            }
        }
        if machine.has_prediction() {
            machine.check_stale(now);
        }
        let position = hbt.position(0);
        metrics.observe(machine.current(), position, hbt.orders(0));
        if iteration % 10 == 0 {
            tracing::info!(
                regime = ?machine.current(),
                regime_version = machine.version(),
                risk_off_reason = ?machine.risk_off_reason(),
                prediction = ?machine.last_prediction(),
                position,
                direction_violations = metrics.direction_violations,
                countertrend_open_qty = metrics.countertrend_open_qty,
                long_to_short_reversals = metrics.long_to_short_reversals,
                short_to_long_reversals = metrics.short_to_long_reversals,
                "regime grid state"
            );
        }
        let policy = DirectionPolicy::resolve(machine.current(), position);
        let forbidden_pending = has_forbidden_pending(hbt.orders(0), policy);
        let prediction = machine.last_prediction();
        let max_long = directional_limit(
            machine.current(),
            prediction,
            config.max_long,
            config.min_directional_limit_ratio,
            RegimeConfig::default().trend_enter_probability,
        );
        let max_short = directional_limit(
            machine.current(),
            prediction,
            config.max_short,
            config.min_directional_limit_ratio,
            RegimeConfig::default().trend_enter_probability,
        );
        let budgets = Budgets::calculate(position, max_long, max_short, &HashMap::new());
        let grid_interval = ((mid * config.relative_grid_interval / min_grid_step).round()
            * min_grid_step)
            .max(min_grid_step);
        let buy_intent = if position < 0.0 {
            OrderIntent::ReduceShort
        } else {
            OrderIntent::OpenLong
        };
        let sell_intent = if position > 0.0 {
            OrderIntent::ReduceLong
        } else {
            OrderIntent::OpenShort
        };
        let horizon_volatility = latest_features
            .map(|features| {
                features[7]
                    * (config.prediction_horizon_ms as f64 / 30_000.0)
                        .max(0.0)
                        .sqrt()
            })
            .unwrap_or(0.0);
        let forecast_mid = forecast_mid_price(
            mid,
            prediction,
            horizon_volatility,
            config.alpha_multiplier,
            grid_interval,
            config.alpha_max_grid_intervals,
        );
        let target = target_position(prediction, max_long, max_short, config.edge_full);
        let normalized_inventory_error = (position - target) / config.order_qty;
        let bid_depth = if buy_intent.is_open() {
            (config.relative_half_spread + config.inventory_skew * normalized_inventory_error)
                .max(0.0)
        } else {
            config.relative_half_spread * config.reduce_spread_factor.clamp(0.0, 1.0)
        };
        let ask_depth = if sell_intent.is_open() {
            (config.relative_half_spread - config.inventory_skew * normalized_inventory_error)
                .max(0.0)
        } else {
            config.relative_half_spread * config.reduce_spread_factor.clamp(0.0, 1.0)
        };
        let bid = ((forecast_mid * (1.0 - bid_depth)).min(best_bid) / grid_interval).floor()
            * grid_interval;
        let ask = ((forecast_mid * (1.0 + ask_depth)).max(best_ask) / grid_interval).ceil()
            * grid_interval;
        let open_levels = if forbidden_pending {
            0
        } else {
            match machine.current() {
                Regime::Sideways => config.sideways_levels,
                Regime::Up | Regime::Down => config.trend_levels,
                Regime::Uncertain | Regime::RiskOff => 0,
            }
        };
        let buy_levels = if buy_intent.is_open() {
            open_levels
        } else {
            config.reduce_levels
        };
        let sell_levels = if sell_intent.is_open() {
            open_levels
        } else {
            config.reduce_levels
        };

        let desired_buy = desired_grid(
            buy_intent,
            bid,
            -grid_interval,
            buy_levels,
            config.order_qty,
            budgets,
            policy,
            tick_size,
        );
        let desired_sell = desired_grid(
            sell_intent,
            ask,
            grid_interval,
            sell_levels,
            config.order_qty,
            budgets,
            policy,
            tick_size,
        );
        reconcile_side(hbt, Side::Buy, desired_buy);
        reconcile_side(hbt, Side::Sell, desired_sell);
        order_registry.sync(hbt.orders(0), machine.current(), machine.version());
        hbt.clear_inactive_orders(Some(0));
    }
    Ok(())
}

fn desired_grid(
    intent: OrderIntent,
    first_price: f64,
    step: f64,
    levels: usize,
    order_qty: f64,
    budgets: Budgets,
    policy: DirectionPolicy,
    tick_size: f64,
) -> HashMap<u64, (f64, f64)> {
    let mut desired = HashMap::new();
    if !policy.allows(intent) || !first_price.is_finite() {
        return desired;
    }
    let mut remaining = budgets.for_intent(intent);
    let mut price = first_price;
    for _ in 0..levels {
        let qty = order_qty.min(remaining);
        if qty <= 0.0 {
            break;
        }
        let tick = (price / tick_size).round() as i64;
        if let Some(id) = encode_order_id(intent, tick) {
            desired.insert(id, (price, qty));
        }
        remaining -= qty;
        price += step;
    }
    desired
}

fn cancel_opening_orders<MD, I>(hbt: &mut I)
where
    MD: MarketDepth,
    I: Bot<MD>,
    I::Error: Debug,
{
    let ids: Vec<_> = hbt
        .orders(0)
        .values()
        .filter(|order| {
            order.cancellable()
                && decode_order_id(order.order_id).is_none_or(|(intent, _)| intent.is_open())
        })
        .map(|order| order.order_id)
        .collect();
    for id in ids {
        hbt.cancel(0, id, false).unwrap();
    }
}

fn reconcile_side<MD, I>(hbt: &mut I, side: Side, desired: HashMap<u64, (f64, f64)>)
where
    MD: MarketDepth,
    I: Bot<MD>,
    I::Error: Debug,
{
    let cancel: Vec<_> = hbt
        .orders(0)
        .values()
        .filter(|order| {
            order.side == side && order.cancellable() && !desired.contains_key(&order.order_id)
        })
        .map(|order| order.order_id)
        .collect();
    let stale_or_forbidden_is_pending = hbt.orders(0).values().any(|order| {
        order.side == side
            && (order.active() || order.req != Status::None)
            && !desired.contains_key(&order.order_id)
    });
    let submit: Vec<_> = desired
        .into_iter()
        .filter(|(id, _)| !hbt.orders(0).contains_key(id))
        .collect();
    // Forbidden/stale orders are always cancelled before replacement orders are submitted.
    for id in cancel {
        hbt.cancel(0, id, false).unwrap();
    }
    // A pending cancellation still consumes exposure. Do not replace it until it disappears.
    if stale_or_forbidden_is_pending {
        return;
    }
    for (id, (price, qty)) in submit {
        let intent = decode_order_id(id)
            .expect("strategy order ID must encode intent")
            .0;
        hbt.submit_order(
            0,
            OrderRequest {
                order_id: id,
                price,
                qty,
                side,
                time_in_force: TimeInForce::GTX,
                order_type: OrdType::Limit,
                reduce_only: !intent.is_open(),
                position_side: PositionSide::Both,
            },
            false,
        )
        .unwrap();
    }
}

#[allow(dead_code)]
fn main() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn immediate_config() -> RegimeConfig {
        RegimeConfig {
            confirmation_count: 2,
            minimum_hold_ns: 0,
            ..Default::default()
        }
    }

    #[test]
    fn state_requires_confirmation_and_opposite_transition_is_uncertain() {
        let mut machine = RegimeMachine::new(immediate_config());
        let up = Prediction {
            up: 0.8,
            sideways: 0.1,
            down: 0.1,
        };
        assert_eq!(machine.update(up, 1), Regime::Uncertain);
        assert_eq!(machine.update(up, 2), Regime::Up);
        let down = Prediction {
            up: 0.1,
            sideways: 0.1,
            down: 0.8,
        };
        assert_eq!(machine.update(down, 3), Regime::Uncertain);
        assert_eq!(machine.update(down, 4), Regime::Uncertain);
        assert_eq!(machine.update(down, 5), Regime::Down);
    }

    #[test]
    fn policy_blocks_countertrend_opening_but_preserves_reducing() {
        let up_short = DirectionPolicy::resolve(Regime::Up, -2.0);
        assert!(!up_short.open_long, "short must be reduced to zero first");
        assert!(up_short.reduce_short);
        assert!(!up_short.open_short);
        let down_long = DirectionPolicy::resolve(Regime::Down, 2.0);
        assert!(down_long.reduce_long);
        assert!(!down_long.open_long);
        assert!(!down_long.open_short, "long must be reduced to zero first");
    }

    #[test]
    fn uncertain_and_risk_off_never_allow_opening() {
        for regime in [Regime::Uncertain, Regime::RiskOff] {
            let policy = DirectionPolicy::resolve(regime, 0.0);
            assert!(!policy.open_long && !policy.open_short);
        }
    }

    #[test]
    fn reduce_budget_never_exceeds_position() {
        let mut pending = HashMap::new();
        pending.insert(OrderIntent::ReduceLong, 1.25);
        let budgets = Budgets::calculate(2.0, 10.0, 10.0, &pending);
        assert_eq!(budgets.reduce_long, 0.75);
        pending.insert(OrderIntent::ReduceLong, 3.0);
        assert_eq!(
            Budgets::calculate(2.0, 10.0, 10.0, &pending).reduce_long,
            0.0
        );
    }

    #[test]
    fn order_id_round_trips_intent_and_price() {
        for intent in [
            OrderIntent::OpenLong,
            OrderIntent::ReduceShort,
            OrderIntent::OpenShort,
            OrderIntent::ReduceLong,
        ] {
            let id = encode_order_id(intent, 123_456).unwrap();
            assert_eq!(decode_order_id(id), Some((intent, 123_456)));
        }
    }

    #[test]
    fn stale_or_invalid_prediction_enters_risk_off() {
        let mut machine = RegimeMachine::new(immediate_config());
        assert_eq!(machine.check_stale(0), Regime::RiskOff);
        let invalid = Prediction {
            up: f64::NAN,
            sideways: 0.5,
            down: 0.5,
        };
        assert_eq!(machine.update(invalid, 1), Regime::RiskOff);
        assert_eq!(
            machine.risk_off_reason(),
            Some(RiskOffReason::InvalidPrediction)
        );
    }

    #[test]
    fn metrics_detect_forbidden_fills_and_direct_reversals_once() {
        let mut metrics = StrategyMetrics::default();
        let id = encode_order_id(OrderIntent::OpenShort, 100).unwrap();
        let mut order = Order::new(
            id,
            100,
            1.0,
            2.0,
            Side::Sell,
            OrdType::Limit,
            TimeInForce::GTX,
        );
        order.exec_qty = 0.5;
        order.exch_timestamp = 10;
        let orders = HashMap::from([(id, order)]);
        metrics.observe(Regime::Up, 1.0, &orders);
        metrics.observe(Regime::Up, -0.5, &orders);
        assert_eq!(metrics.direction_violations, 1);
        assert_eq!(metrics.countertrend_open_qty, 0.5);
        assert_eq!(metrics.long_to_short_reversals, 1);
    }

    #[test]
    fn pending_forbidden_order_blocks_all_new_opening() {
        let id = encode_order_id(OrderIntent::OpenLong, 100).unwrap();
        let mut order = Order::new(
            id,
            100,
            1.0,
            1.0,
            Side::Buy,
            OrdType::Limit,
            TimeInForce::GTX,
        );
        order.status = Status::New;
        order.req = Status::Canceled;
        let orders = HashMap::from([(id, order)]);
        let down_policy = DirectionPolicy::resolve(Regime::Down, 0.0);
        assert!(has_forbidden_pending(&orders, down_policy));
    }

    #[test]
    fn recovered_reduce_orders_cannot_exceed_position() {
        let id = encode_order_id(OrderIntent::ReduceLong, 101).unwrap();
        let mut order = Order::new(
            id,
            101,
            1.0,
            3.0,
            Side::Sell,
            OrdType::Limit,
            TimeInForce::GTX,
        );
        order.status = Status::New;
        let orders = HashMap::from([(id, order)]);
        let mut registry = OrderRegistry::default();
        registry.recover(&orders).unwrap();
        assert_eq!(registry.get(id).unwrap().intent, OrderIntent::ReduceLong);
        assert!(!order_exposure_consistent(2.0, &orders));
        assert!(order_exposure_consistent(3.0, &orders));
    }

    #[test]
    fn prediction_controls_target_limit_and_bounded_quote_center() {
        let up = Prediction {
            up: 0.8,
            sideways: 0.15,
            down: 0.05,
        };
        assert_eq!(target_position(Some(up), 10.0, 8.0, 0.5), 10.0);
        let down = Prediction {
            up: 0.05,
            sideways: 0.15,
            down: 0.8,
        };
        assert_eq!(target_position(Some(down), 10.0, 8.0, 0.5), -8.0);
        let limit = directional_limit(Regime::Up, Some(up), 10.0, 0.5, 0.62);
        assert!((5.0..=10.0).contains(&limit));
        let shifted = forecast_mid_price(100.0, Some(up), 1.0, 10.0, 0.5, 1.0);
        assert!(shifted > 100.0);
        assert!(shifted <= 100.0 * 0.005_f64.exp() + 1e-12);
    }

    #[test]
    fn volatility_shock_compares_horizon_scaled_windows() {
        let mut features = [0.0; FEATURE_NAMES.len()];
        features[8] = 0.01;
        features[6] = 0.001;
        assert!(!volatility_shock(Some(features), 4.0));
        features[6] = 0.01;
        assert!(volatility_shock(Some(features), 4.0));
        assert!(!volatility_shock(None, 4.0));
    }

    fn model_json(std_override: Option<f64>, hash: &str) -> String {
        let features = serde_json::to_string(&FEATURE_NAMES).unwrap();
        let mean = serde_json::to_string(&vec![0.0; FEATURE_NAMES.len()]).unwrap();
        let mut std = vec![1.0; FEATURE_NAMES.len()];
        if let Some(value) = std_override {
            std[0] = value;
        }
        let std = serde_json::to_string(&std).unwrap();
        let mut up = vec![0.0; FEATURE_NAMES.len()];
        up[0] = 2.0;
        let up = serde_json::to_string(&up).unwrap();
        let mut down = vec![0.0; FEATURE_NAMES.len()];
        down[0] = -2.0;
        let down = serde_json::to_string(&down).unwrap();
        format!(
            r#"{{
                "model_type":"multinomial_group_lasso",
                "version":"test-v1",
                "prediction_horizon_ms":60000,
                "feature_schema_hash":"{hash}",
                "features":{features},
                "mean":{mean},
                "std":{std},
                "intercept_up":0.0,
                "intercept_down":0.0,
                "coef_up":{up},
                "coef_down":{down},
                "temperature":1.0
            }}"#
        )
    }

    #[test]
    fn model_validates_schema_and_predicts_stable_softmax() {
        let model =
            MultinomialModel::from_json(&model_json(None, &feature_schema_hash()), 60_000).unwrap();
        let mut features = [0.0; FEATURE_NAMES.len()];
        features[0] = 1.0;
        let prediction = model.predict(&features).unwrap();
        assert!(prediction.up > prediction.sideways);
        assert!(prediction.sideways > prediction.down);
        assert_eq!(model.version, "test-v1");
    }

    #[test]
    fn rust_probability_matches_training_reference() {
        let mut coef_up = vec![0.0; FEATURE_NAMES.len()];
        let mut coef_down = vec![0.0; FEATURE_NAMES.len()];
        coef_up[0] = 0.3;
        coef_down[0] = -0.4;
        let model = MultinomialModel {
            version: "parity".to_owned(),
            prediction_horizon_ms: 60_000,
            mean: vec![0.0; FEATURE_NAMES.len()],
            std: vec![1.0; FEATURE_NAMES.len()],
            intercept_up: 0.1,
            intercept_down: -0.2,
            coef_up,
            coef_down,
            temperature: 1.0,
        };
        let mut features = [0.0; FEATURE_NAMES.len()];
        features[0] = 0.5;
        let prediction = model.predict(&features).unwrap();
        // Generated by regime_model.fista.predict from the same exported parameters.
        assert!((prediction.up - 0.43462263736215145).abs() < 1e-14);
        assert!((prediction.sideways - 0.3384844503182028).abs() < 1e-14);
        assert!((prediction.down - 0.22689291231964576).abs() < 1e-14);
    }

    #[test]
    fn model_rejects_schema_horizon_and_standardization_mismatches() {
        assert!(MultinomialModel::from_json(&model_json(None, "sha256:wrong"), 60_000).is_err());
        assert!(
            MultinomialModel::from_json(&model_json(None, &feature_schema_hash()), 30_000).is_err()
        );
        assert!(
            MultinomialModel::from_json(&model_json(Some(0.0), &feature_schema_hash()), 60_000)
                .is_err()
        );
    }

    #[test]
    fn feature_engine_uses_only_closed_history_and_emits_all_features() {
        let mut engine = FeatureEngine::new(1_000_000_000);
        let mut depth = HashMapMarketDepth::new(0.01, 1.0);
        for second in 0..=301_i64 {
            let bid = 100.0 + second as f64 * 0.001;
            depth.update_bid_depth(bid, 10.0, second * 1_000_000_000);
            depth.update_ask_depth(bid + 0.02, 8.0, second * 1_000_000_000);
            assert!(engine.sample(&depth, second * 1_000_000_000));
        }
        let snapshot = engine.snapshot().unwrap();
        assert_eq!(snapshot.len(), FEATURE_NAMES.len());
        assert!(snapshot.iter().all(|value| value.is_finite()));
        assert!(snapshot[3] > 0.0, "60-second return should detect the rise");
        assert!(snapshot[12] > 0.0, "bid-heavy L1 should be positive");
    }
}
