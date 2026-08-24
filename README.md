# HFT Backtest

面向 **Binance USD-M Futures** 的 Rust 订单簿高频交易项目。

核心能力：L2 OrderBook、Trade Tick、Queue Position、Latency、Partial Fill、Fee、Position/PnL；交易所 I/O 全部位于 `app`，纯计算、策略和回测位于 `engine`。

```text
Binance Public WS ─┐
                   ├─ Normalized Event ─ L2 MarketState ─ Strategy ─ Risk ─ Live Executor ─ Binance
Binance Private WS ┘                         ↑                                   │
                                             └──── Order / Position 回报 ─────────┘

Backtest:
Normalized Event ─ L2 OrderBook ─ Strategy ─ Queue / Latency / Partial Fill / Fee
```

## 环境

```text
Rust 1.91.1
Linux / macOS
```

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

# 配置

测试环境模板：

```bash
cp app/examples/binancefutures-demo.toml.example app/examples/local.toml
```

`local.toml` 同时包含 Binance、策略和风险配置：

```toml
public_stream_url = "wss://stream.binancefuture.com/ws"
private_stream_url = "wss://stream.binancefuture.com/ws/{listen_key}"
api_url = "https://testnet.binancefuture.com"

order_prefix = "hfttest"
api_key = ""
secret = ""

[risk]
max_order_qty = 0.001
max_order_notional = 100.0
max_position = 0.003
max_open_orders = 4

[[strategies]]
symbol = "BTCUSDT"
kind = "grid"
tick_size = 0.1
lot_size = 0.001
relative_half_spread = 0.0005
relative_grid_interval = 0.0005
grid_num = 2
min_grid_step = 0.1
skew = 0.00025
order_qty = 0.001
max_position = 0.003
```

字段边界：

- Binance 层只负责 WS / REST / Order Manager。
- `[[strategies]]` 是 Runtime 唯一的 symbol 和策略配置来源。
- `[risk]` 是所有 Live StrategyCommand 发往交易所前的最终闸门。
- 当前一个 symbol 只允许配置一个内建策略；未来新增策略时扩展 `BuiltinStrategy`，不修改 Binance Adapter。
- `tick_size`、`lot_size` 必须与交易所当前合约规则一致。

不要提交包含真实 API Key/Secret 的 `local.toml`。

# 目录与边界

```text
engine/                         纯计算、策略与回测
app/src/config.rs               聚合配置和启动校验
app/src/ports.rs                行情源 / 执行通道接口
app/src/live/                   Runtime、风控与执行编排
app/src/exchange/binance_usdm/  Binance WS/REST/协议实现
app/src/bin/collector.rs        只读行情采集入口
```

依赖方向是 `app -> engine`；`live` 与 Binance Adapter 通过 `MarketDataSource` / `ExecutionVenue` 解耦。完整说明见 `docs/architecture.md`。

# 内建策略

当前从 `master` 迁移并维护：

```text
Grid Market Making
```

策略代码位于：

```text
engine/src/strategy/grid.rs
```

统一接口：

```text
MarketContext
    ↓
Strategy
    ↓
StrategyCommand
    ├─ Submit
    ├─ Modify
    └─ Cancel
```

策略模块禁止依赖 Binance、HTTP、WebSocket、API Key 或 Tokio。

# Runtime：Dry-run

默认模式只运行真实行情、L2 MarketState、策略决策和 RiskGate，**不会向 Binance 发订单**：

```bash
RUST_LOG=debug cargo run -p hft-app -- \
  app/examples/local.toml
```

Runtime 会自动读取配置文件里全部 `[[strategies]]`，并订阅其中配置的 symbol。CLI 不再重复接收 symbol 参数。

Dry-run 日志会出现：

```text
strategy command generated (dry-run)
```

用于先验证 Grid 策略会产生哪些挂单/撤单命令。

# Runtime：Binance Testnet 实际执行

先在 `app/examples/local.toml` 填入 **USD-M Futures Testnet** API Key / Secret，然后显式增加 `--execute`：

```bash
RUST_LOG=debug cargo run -p hft-app -- \
  app/examples/local.toml \
  --execute
```

`--execute` 是硬安全开关：

- 未提供时绝不调用 Submit/Cancel。
- API Key/Secret 为空时拒绝启动执行模式。
- 执行模式会等待 Binance 初始 Position 同步完成后才允许策略下单。
- User Data Stream 断开会立即暂停执行，逐个 symbol 重新完成 Position 对账后才恢复。
- StrategyCommand 先经过 RiskGate，再进入 Binance Connector。
- Submit 会先写入本地 pending order，防止 REST/WS 回报返回前重复提交同一 Grid 订单。
- Binance `Order` / `Position` 回报会写回 LiveStrategyRuntime，下一轮策略直接使用最新状态。
- Ctrl+C 时会撤销本进程已跟踪的活动订单并等待确认；超时会明确告警。

完整闭环：

```text
Public WS Depth/Trade
        ↓
Normalized Event
        ↓
L2 MarketState
        ↓
GridStrategy
        ↓
StrategyCommand
        ↓
RiskGate
        ↓
Submit / Cancel
        ↓
Binance Testnet REST
        ↓
Private User Stream
        ↓
Order / Position
        ↓
LiveStrategyRuntime
        └────────────→ 下一次策略决策
```

# Collector

Collector 与 Live 共用同一个 Binance Market Adapter：

```bash
mkdir -p data
cargo run -p hft-app --bin collector -- \
  app/examples/local.toml \
  data/btcusdt.csv \
  BTCUSDT
```

Collector 不依赖 `[[strategies]]`，因此仍显式接收采集 symbol 和输出文件路径。

Collector 不启动 User Data Stream，也不需要 API Key。

CSV：

```text
symbol,ev,exch_ts,local_ts,px,qty,order_id,ival,fval
```

启动后立即 flush 表头，运行期间每秒 flush 一次。

# 回测

回测目前是 `engine` Rust Library 能力，没有独立 CLI。

```text
Normalized Event
  → Replay
  → L2 OrderBook
  → Strategy
  → Order Latency
  → Queue
  → Partial Fill
  → Fee
  → Position/PnL
```

默认执行模型：`PartialFillExchange`。

```bash
cargo test -p hftbacktest
cargo test -p hftbacktest --test golden_l2
```

GridStrategy 使用和 Live 相同的 `Strategy` / `StrategyCommand` 语义。

# Binance 接口基线

```text
Depth cadence    -> @depth@100ms
Depth snapshot   -> GET /fapi/v1/depth
Position         -> GET /fapi/v3/positionRisk
New order        -> POST /fapi/v1/order
Cancel order     -> DELETE /fapi/v1/order
User stream      -> POST/PUT /fapi/v1/listenKey
```

本地 L2 OrderBook 使用 REST Snapshot + Diff Depth `U/u/pu` 连续性规则；发现断档立即重新同步。

主动执行面保持最小：Live 仅执行 Submit/Cancel，订单类型限定为 `LIMIT/MARKET` 与 `GTC/IOC/FOK/GTX`。`StrategyCommand::Modify` 目前只在 Engine/回测侧有语义，Live 会明确拒绝。Binance Algo conditional orders 不属于当前项目目标。

生产配置采用 Binance 当前 routed WebSocket 路径；Demo 模板使用 `demo-fstream.binance.com` 与 `demo-fapi.binance.com`。端点变更应以 Binance 官方文档为准。

# Release

```bash
cargo build --release --workspace --locked
```

Dry-run：

```bash
./target/release/hft-app app/examples/local.toml
```

Testnet execute：

```bash
./target/release/hft-app app/examples/local.toml --execute
```

Collector：

```bash
./target/release/collector app/examples/local.toml data/btcusdt.csv BTCUSDT
```

# 推荐验证顺序

```text
check / test / clippy
        ↓
Collector + L2 连续性
        ↓
Runtime Dry-run
        ↓
确认 StrategyCommand
        ↓
Testnet + --execute
        ↓
确认 NEW / CANCELED / FILLED / Position
        ↓
断线与重同步测试
        ↓
最后才考虑 Mainnet
```

# 项目边界

保留：

```text
Binance USD-M Futures
L2 OrderBook / Trade Tick
Queue / Latency / Partial Fill
Fee / Position / PnL
Strategy API / GridStrategy
RiskGate / Live Executor
Collector / Live Adapter
```

不包含：

```text
Python / AWS S3
L3 Market-By-Order
Bybit / Spot / Hyperliquid
Iceoryx IPC
Binance Algo conditional-order subsystem
```

进一步策略扩展规则见 `docs/strategy.md`。
