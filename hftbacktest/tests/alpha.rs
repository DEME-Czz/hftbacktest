use std::collections::HashMap;

use hftbacktest::{
    alpha::{
        AlphaConfig, AlphaEngine, AlphaModel, AlphaPrediction, CsvDatasetWriter, Direction,
        FEATURE_COUNT, FeatureStandardizer, LINEAR_INPUT_COUNT, LabelConfig, LinearAlphaModel,
        LobRecord, LobSnapshot, LobWindow, TrainingConfig, WINDOW_SIZE, label_records,
        load_csv_records,
    },
    depth::{HashMapMarketDepth, L2MarketDepth, MarketDepth},
};
use std::io::Cursor;

#[derive(Default)]
struct TestDepth {
    bids: HashMap<i64, f64>,
    asks: HashMap<i64, f64>,
    best_bid: i64,
    best_ask: i64,
    tick_size: f64,
}

impl TestDepth {
    fn ten_levels() -> Self {
        let bids = (0..10).map(|i| (100 - i, 10.0 + i as f64)).collect();
        let asks = (0..10).map(|i| (101 + i, 20.0 + i as f64)).collect();
        Self {
            bids,
            asks,
            best_bid: 100,
            best_ask: 101,
            tick_size: 0.01,
        }
    }
}

impl MarketDepth for TestDepth {
    fn best_bid(&self) -> f64 {
        self.best_bid as f64 * self.tick_size
    }

    fn best_ask(&self) -> f64 {
        self.best_ask as f64 * self.tick_size
    }

    fn best_bid_tick(&self) -> i64 {
        self.best_bid
    }

    fn best_ask_tick(&self) -> i64 {
        self.best_ask
    }

    fn best_bid_qty(&self) -> f64 {
        self.bid_qty_at_tick(self.best_bid)
    }

    fn best_ask_qty(&self) -> f64 {
        self.ask_qty_at_tick(self.best_ask)
    }

    fn tick_size(&self) -> f64 {
        self.tick_size
    }

    fn lot_size(&self) -> f64 {
        1.0
    }

    fn bid_qty_at_tick(&self, price_tick: i64) -> f64 {
        self.bids.get(&price_tick).copied().unwrap_or(0.0)
    }

    fn ask_qty_at_tick(&self, price_tick: i64) -> f64 {
        self.asks.get(&price_tick).copied().unwrap_or(0.0)
    }
}

#[test]
fn extracts_ten_levels_in_ask_price_qty_bid_price_qty_order() {
    let snapshot = LobSnapshot::from_depth(&TestDepth::ten_levels()).unwrap();

    assert_eq!(snapshot.features().len(), FEATURE_COUNT);
    assert_eq!(&snapshot.features()[0..4], &[1.01, 20.0, 1.0, 10.0]);
    assert_eq!(&snapshot.features()[36..40], &[1.1, 29.0, 0.91, 19.0]);
}

#[test]
fn rejects_an_order_book_without_ten_levels_on_each_side() {
    let mut depth = TestDepth::ten_levels();
    depth.asks.remove(&110);

    let error = LobSnapshot::from_depth(&depth).unwrap_err();

    assert!(error.to_string().contains("ask"));
}

#[test]
fn window_keeps_one_hundred_distinct_states_and_ignores_duplicates() {
    let mut window = LobWindow::new();
    let mut depth = TestDepth::ten_levels();

    let first = LobSnapshot::from_depth(&depth).unwrap();
    assert!(window.push(first.clone()));
    assert!(!window.push(first));

    for index in 1..=WINDOW_SIZE {
        depth.bids.insert(100, 10.0 + index as f64);
        assert!(window.push(LobSnapshot::from_depth(&depth).unwrap()));
    }

    assert_eq!(window.len(), WINDOW_SIZE);
    assert!(window.is_ready());
    assert_eq!(window.latest().unwrap().features()[3], 110.0);
}

struct FixedModel(AlphaPrediction);

impl AlphaModel for FixedModel {
    type Error = std::convert::Infallible;

    fn predict(&mut self, input: &LobWindow) -> Result<AlphaPrediction, Self::Error> {
        assert!(input.is_ready());
        Ok(self.0)
    }
}

#[test]
fn converts_a_confident_up_prediction_into_a_bounded_price_offset() {
    let model = FixedModel(AlphaPrediction::new(0.05, 0.10, 0.85).unwrap());
    let config = AlphaConfig {
        confidence_threshold: 0.60,
        calibrated_return: 0.002,
        max_relative_offset: 0.001,
        smoothing: 1.0,
    };
    let mut engine = AlphaEngine::new(model, config).unwrap();
    let mut depth = TestDepth::ten_levels();

    for index in 0..WINDOW_SIZE {
        depth.bids.insert(100, 10.0 + index as f64);
        engine.update(&depth).unwrap();
    }

    let signal = engine.latest_signal();
    assert_eq!(signal.direction, Direction::Up);
    assert!((signal.price_offset - 0.001005).abs() < 1e-9);
}

#[test]
fn low_confidence_prediction_falls_back_to_flat_and_zero_offset() {
    let model = FixedModel(AlphaPrediction::new(0.30, 0.36, 0.34).unwrap());
    let mut engine = AlphaEngine::new(model, AlphaConfig::default()).unwrap();
    let mut depth = TestDepth::ten_levels();

    for index in 0..WINDOW_SIZE {
        depth.asks.insert(101, 20.0 + index as f64);
        engine.update(&depth).unwrap();
    }

    assert_eq!(engine.latest_signal().direction, Direction::Flat);
    assert_eq!(engine.latest_signal().price_offset, 0.0);
}

#[test]
fn rejects_invalid_class_probabilities() {
    assert!(AlphaPrediction::new(0.5, 0.5, 0.5).is_err());
    assert!(AlphaPrediction::new(f32::NAN, 0.5, 0.5).is_err());
}

#[test]
fn csv_writer_emits_a_stable_header_and_ignores_duplicate_book_states() {
    let depth = TestDepth::ten_levels();
    let record = LobRecord::from_depth(123_000_000, &depth).unwrap();
    let mut writer = CsvDatasetWriter::new(Vec::new()).unwrap();

    assert!(writer.write(&record).unwrap());
    assert!(!writer.write(&record).unwrap());
    let bytes = writer.into_inner();
    let csv = String::from_utf8(bytes).unwrap();
    let rows: Vec<_> = csv.lines().collect();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].split(',').count(), 42);
    assert_eq!(rows[1].split(',').count(), 42);
    assert!(rows[0].starts_with("exchange_timestamp,mid_price,ask_price_1,ask_qty_1"));
    assert!(rows[1].starts_with("123000000,1.005"));
}

fn records_with_mid_prices(prices: impl IntoIterator<Item = f64>) -> Vec<LobRecord> {
    let snapshot = LobSnapshot::from_depth(&TestDepth::ten_levels()).unwrap();
    prices
        .into_iter()
        .enumerate()
        .map(|(index, mid_price)| {
            LobRecord::new(index as i64, mid_price, snapshot.clone()).unwrap()
        })
        .collect()
}

#[test]
fn reads_collected_csv_and_rejects_wrong_column_counts() {
    let mut row = String::from("exchange_timestamp,mid_price");
    for level in 1..=10 {
        row.push_str(&format!(
            ",ask_price_{level},ask_qty_{level},bid_price_{level},bid_qty_{level}"
        ));
    }
    row.push('\n');
    row.push_str("1,1.0");
    for value in 0..FEATURE_COUNT {
        row.push_str(&format!(",{}", value + 1));
    }
    row.push('\n');

    let records = load_csv_records(Cursor::new(row)).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].snapshot().features()[39], 40.0);

    assert!(load_csv_records(Cursor::new("exchange_timestamp,mid_price\n1,1\n")).is_err());
}

#[test]
fn csv_reader_ignores_only_an_unterminated_partial_tail() {
    let mut csv = String::from("exchange_timestamp,mid_price");
    for level in 1..=10 {
        csv.push_str(&format!(
            ",ask_price_{level},ask_qty_{level},bid_price_{level},bid_qty_{level}"
        ));
    }
    csv.push('\n');
    csv.push_str("1,1.0");
    for value in 0..FEATURE_COUNT {
        csv.push_str(&format!(",{}", value + 1));
    }
    csv.push_str("\n2,1.0,1,2");

    assert_eq!(load_csv_records(Cursor::new(csv)).unwrap().len(), 1);
}

#[test]
fn standardizer_uses_training_values_and_handles_constant_features() {
    let mut first = [1.0; FEATURE_COUNT];
    let mut second = [1.0; FEATURE_COUNT];
    first[0] = 10.0;
    second[0] = 14.0;
    let scaler = FeatureStandardizer::fit(&[first, second]).unwrap();

    assert_eq!(scaler.transform_value(0, 12.0), 0.0);
    assert_eq!(scaler.transform_value(1, 1.0), 0.0);
    assert_eq!(scaler.transform_value(0, 14.0), 1.0);
}

#[test]
fn serialized_linear_model_produces_three_probabilities_after_reload() {
    let scaler = FeatureStandardizer::identity();
    let mut weights = vec![0.0; 3 * LINEAR_INPUT_COUNT];
    weights[2 * LINEAR_INPUT_COUNT] = 2.0;
    let model = LinearAlphaModel::new(scaler, weights, [0.0, 0.0, 0.0]).unwrap();
    let encoded = model.to_json().unwrap();
    let mut restored = LinearAlphaModel::from_json(&encoded).unwrap();
    let mut window = LobWindow::new();
    for index in 0..WINDOW_SIZE {
        let mut features = [0.0; FEATURE_COUNT];
        features[0] = if index == 0 { 1.0 } else { 0.0 };
        features[1] = index as f32;
        window.push(LobSnapshot::new(features).unwrap());
    }

    let prediction = restored.predict(&window).unwrap();
    assert!(prediction.up() > prediction.flat());
    assert!((prediction.down() + prediction.flat() + prediction.up() - 1.0).abs() < 1e-5);
}

#[test]
fn training_config_rejects_leaky_or_empty_splits() {
    assert!(TrainingConfig::new(0.0, 3, 0.01, 0.0001).is_err());
    assert!(TrainingConfig::new(1.0, 3, 0.01, 0.0001).is_err());
    assert!(TrainingConfig::new(0.8, 0, 0.01, 0.0001).is_err());
    assert!(TrainingConfig::new(0.8, 3, 0.01, 0.0001).is_ok());
}

#[test]
fn labels_up_down_and_flat_using_smoothed_past_and_future_mid_prices() {
    let config = LabelConfig::new(2, 0.005).unwrap();
    let rising = records_with_mid_prices((0..110).map(|index| 100.0 + index as f64));
    let falling = records_with_mid_prices((0..110).map(|index| 300.0 - index as f64));
    let flat = records_with_mid_prices(std::iter::repeat_n(100.0, 110));

    let rising_labels = label_records(&rising, config);
    let falling_labels = label_records(&falling, config);
    let flat_labels = label_records(&flat, config);

    assert_eq!(rising_labels.first().unwrap().window_end_index, 99);
    assert!(
        rising_labels
            .iter()
            .all(|label| label.direction == Direction::Up)
    );
    assert!(
        falling_labels
            .iter()
            .all(|label| label.direction == Direction::Down)
    );
    assert!(
        flat_labels
            .iter()
            .all(|label| label.direction == Direction::Flat)
    );
}

#[test]
fn label_config_rejects_zero_horizon_and_invalid_threshold() {
    assert!(LabelConfig::new(0, 0.001).is_err());
    assert!(LabelConfig::new(10, -0.001).is_err());
    assert!(LabelConfig::new(10, f64::NAN).is_err());
}

#[test]
fn hashmap_depth_retains_the_latest_exchange_timestamp() {
    let mut depth = HashMapMarketDepth::new(0.01, 1.0);

    depth.update_bid_depth(1.0, 10.0, 200);
    depth.update_ask_depth(1.01, 20.0, 150);

    assert_eq!(depth.timestamp, 200);
}
