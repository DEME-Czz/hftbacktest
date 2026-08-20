# 方向感知网格策略使用指南

本仓库在 `hftbacktest` 原有撮合、延迟和队列模型之上增加行情状态感知与开仓方向控制。策略可在回测中使用纯算法判断上涨、震荡和下跌；模型属于可选增强能力，不是运行回测的必要条件。

## 核心逻辑

纯算法分类器每秒计算指定观察窗口内的中间价对数收益：

```text
score = log(current_mid / historical_mid) / trend_return_threshold
```

该分数被转换为 `P(up)`、`P(sideways)` 和 `P(down)`，再经过确认和滞回状态机。默认要求连续确认 3 次，状态至少保持 5 秒；上涨与下跌不能直接互相切换，必须经过 `Uncertain`。

| 状态 | 允许的行为 |
|---|---|
| `Up` | 开多、平多、平空；禁止开空 |
| `Down` | 开空、平多、平空；禁止开多 |
| `Sideways` | 双向网格和减仓 |
| `Uncertain` | 禁止开仓，只允许减仓 |
| `RiskOff` | 撤销开仓单，只允许降低风险 |

订单被明确标记为 `OpenLong`、`OpenShort`、`ReduceLong` 或 `ReduceShort`。状态切换后，禁止方向的旧订单必须撤销完毕，策略才会提交新的开仓单。减仓同时受策略数量预算和交易所 `reduceOnly` 保护。

## 构建与测试

需要 Rust 1.91.1 或更新版本和 Python 3.11+：

```bash
cargo build --release -p hftbacktest \
  --example regime_grid_backtest \
  --example regime_feature_dump \
  --example regime_grid_live
cargo build --release -p connector

cargo test -p hftbacktest --example regime_grid_backtest
PYTHONPATH=model-training python3 -m unittest discover -s model-training/tests -v
```

## 纯算法回测

准备 L2 行情 NPZ、订单延迟 NPZ，以及标的正确的价格步长和数量步长：

```bash
mkdir -p runtime/regime-results

./target/release/examples/regime_grid_backtest \
  --name doge_rule_ \
  --output-path runtime/regime-results \
  --data-files data/DOGEUSDT_20260701.npz \
  --latency-files data/latency_20260701.npz \
  --initial-snapshot data/DOGEUSDT_20260701_SOD.npz \
  --tick-size 0.00001 \
  --lot-size 1 \
  --return-horizon-ms 60000 \
  --trend-return-threshold 0.002 \
  --relative-half-spread 0.0005 \
  --relative-grid-interval 0.0005 \
  --order-qty 100 \
  --max-long 1000 \
  --max-short 1000 \
  --max-position-hard 1000
```

不传 `--model` 即启用纯算法。输出为 `runtime/regime-results/doge_rule_0.csv`，包含时间、余额、仓位、手续费、成交量、成交额、成交次数和中间价。

多个连续行情文件可以按时间顺序传入：

```bash
--data-files day1.npz day2.npz day3.npz
```

## 参数说明

### 行情感知

| 参数 | 默认值 | 说明 |
|---|---:|---|
| `--return-horizon-ms` | `60000` | 纯算法观察窗口；越短越灵敏、噪声越大 |
| `--trend-return-threshold` | `0.002` | 一个方向证据单位，`0.002` 约为 0.2% 对数收益 |
| `--model` | 无 | 可选模型 JSON；传入后替代纯算法分类器 |
| `--prediction-horizon-ms` | `60000` | 必须与模型训练周期一致 |

### 网格与仓位

| 参数 | 默认值 | 说明 |
|---|---:|---|
| `--relative-half-spread` | `0.0005` | 第一层报价距预测中心的相对距离 |
| `--relative-grid-interval` | `0.0005` | 网格层间距占中间价的比例 |
| `--sideways-levels` | `10` | 震荡状态单侧开仓层数 |
| `--trend-levels` | `6` | 趋势状态顺势开仓层数 |
| `--reduce-levels` | `4` | 减仓层数 |
| `--order-qty` | `1` | 每层订单数量 |
| `--max-long` / `--max-short` | `10` | 策略方向仓位上限 |
| `--max-position-hard` | `10` | 绝对硬仓位上限，触发后进入 `RiskOff` |
| `--inventory-skew` | 自动 | 根据仓位偏离目标调整两侧报价距离 |
| `--reduce-spread-factor` | `0.7` | 减仓报价距离系数；越小越积极 |

### 风险控制

| 参数 | 默认值 | 说明 |
|---|---:|---|
| `--max-spread-bps` | `20` | 市场价差上限 |
| `--market-data-stale-ms` | `2000` | 深度数据允许的最大延迟 |
| `--max-daily-loss` | `100` | 相对启动余额的最大亏损 |
| `--max-drawdown` | `150` | 相对运行期峰值余额的最大回撤 |
| `--volatility-shock-multiple` | `4` | 5 秒波动相对长期预期的冲击倍数 |
| `--maker-fee` / `--taker-fee` | `-0.00005` / `0.0007` | 回测手续费率 |

参数必须按标的过滤器调整。`tick_size` 是最小价格变化，`lot_size` 是最小数量变化；`order_qty` 还必须满足交易所最小名义价值。

## 如何调节算法敏感度

推荐先只调整两个参数：

```text
识别更快：缩短 return_horizon_ms，降低 trend_return_threshold
识别更稳：延长 return_horizon_ms，提高 trend_return_threshold
```

例如较灵敏配置：

```bash
--return-horizon-ms 30000 --trend-return-threshold 0.001
```

较稳健配置：

```bash
--return-horizon-ms 120000 --trend-return-threshold 0.003
```

应使用不同日期的样本外行情比较净收益、最大回撤、逆势仓位增量和状态切换频率，不能只根据单日收益选参数。

## 可选模型流程

只使用算法时可以跳过本节。模型流程使用与运行时相同的 Rust `FeatureEngine`：

```bash
./target/release/examples/regime_feature_dump \
  --data-files data/DOGEUSDT_TRAIN.npz \
  --initial-snapshot data/DOGEUSDT_TRAIN_SOD.npz \
  --output runtime/train_features.csv \
  --tick-size 0.00001 --lot-size 1

PYTHONPATH=model-training python3 -m regime_model.build_labels \
  --input runtime/train_features.csv \
  --output runtime/train_labeled.csv

PYTHONPATH=model-training python3 -m regime_model.train_glasso \
  --input runtime/train_labeled.csv \
  --validation runtime/validation_labeled.csv \
  --output runtime/regime_model.json
```

随后在回测命令中增加：

```bash
--model runtime/regime_model.json --prediction-horizon-ms 60000
```

训练集和验证集必须按时间分离，不能随机打乱或重叠。

## 实盘与 Demo

当前 `regime_grid_live` 为安全起见要求提供 `--model`；纯算法模式目前只在回测入口开放。运行实盘前应先完成多个时间区间的样本外回测，再使用 Binance Futures Demo 验证。

先启动 connector：

```bash
cp connector/examples/binancefutures-demo.toml.example \
  connector/examples/binancefutures-demo.toml
# 在本地配置中填写 Demo 密钥，并将 name 设置为 binancefutures-demo。

RUST_LOG=info ./target/release/connector \
  binancefutures-demo \
  binancefutures \
  connector/examples/binancefutures-demo.toml
```

另一个终端启动策略：

```bash
RUST_LOG=info ./target/release/examples/regime_grid_live \
  --connector binancefutures-demo \
  --symbol dogeusdt \
  --tick-size 0.00001 \
  --lot-size 1 \
  --model runtime/regime_model.json \
  --order-qty 100 \
  --max-long 500 \
  --max-short 500 \
  --max-position-hard 500
```

策略只支持 Binance Futures 单向持仓模式（`positionSide=BOTH`）。若账户处于 Hedge Mode、账户状态未同步、行情不完整或存在未知订单，策略将拒绝正常开仓或进入 `RiskOff`。

## 运行观察与故障排查

使用 `RUST_LOG=info` 可观察 `regime`、三类概率、仓位、风险原因和方向违规计数。常见 `RiskOff` 原因包括：行情断流、交叉盘口、价差过大、波动冲击、仓位超限、日亏损、最大回撤、模型或预测无效、未知订单及减仓订单超过实际持仓。

查看更多参数：

```bash
./target/release/examples/regime_grid_backtest --help
./target/release/examples/regime_feature_dump --help
./target/release/examples/regime_grid_live --help
```

实盘密钥不得提交到 Git。生产密钥应关闭提现权限、设置 IP 白名单，并从极小仓位开始。
