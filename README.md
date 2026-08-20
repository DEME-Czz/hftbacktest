# HFT Backtest

面向 **Binance USD-M Futures** 的 Rust 订单簿高频交易项目。

核心能力：L2 OrderBook、Trade Tick、Queue Position、Latency、Partial Fill、Fee、Position/PnL；交易所 I/O 全部位于 `app`，纯计算与回测位于 `engine`。

```text
Binance Public WS ─┐
                   ├─ Normalized Event ─ L2 OrderBook ─ Strategy
Binance Private WS ┘                         │
                                             ├─ Backtest: Queue + Latency + Partial Fill + Fee
                                             └─ Live: Binance Order Manager
```

## 环境

```text
Rust 1.91.1
Linux / macOS
```

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

# Binance 配置

> 2026 年 Binance USD-M WebSocket 已迁移到 public/private 分离入口。生产配置不要再使用旧的 `wss://fstream.binance.com/ws` 或 `/stream`。

生产示例：

```toml
public_stream_url = "wss://fstream.binance.com/public/ws"
private_stream_url = "wss://fstream.binance.com/private/ws?listenKey={listen_key}&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE/TRADE_LITE/listenKeyExpired"
api_url = "https://fapi.binance.com"

order_prefix = "hft"
api_key = ""
secret = ""
```

字段：

| 字段 | 说明 |
|---|---|
| `public_stream_url` | 公共市场 WebSocket；高频 L2/Trade 使用 |
| `private_stream_url` | 用户数据 WebSocket URL 模板，必须包含 `{listen_key}` |
| `api_url` | USD-M REST API |
| `order_prefix` | 本系统 client order id 前缀 |
| `api_key` | USER_STREAM/账户/交易接口使用 |
| `secret` | SIGNED REST 请求 HMAC 签名使用 |

仓库配置：

```text
app/examples/binancefutures.toml                 # Mainnet 当前接口
app/examples/binancefutures-demo.toml.example    # Demo 示例
```

建议：

```bash
cp app/examples/binancefutures-demo.toml.example app/examples/local.toml
```

不要提交包含真实 API Key/Secret 的 `local.toml`。

## 当前 Binance 接口基线

当前代码已经按 2026 官方 USD-M 文档收敛到：

```text
Market WS        -> public stream
User Data WS     -> private stream
Depth cadence    -> @depth@100ms
Depth snapshot   -> GET /fapi/v1/depth
Position         -> GET /fapi/v3/positionRisk
New order        -> POST /fapi/v1/order
Modify order     -> PUT /fapi/v1/order
Cancel order     -> DELETE /fapi/v1/order
Batch order      -> POST /fapi/v1/batchOrders
Batch cancel     -> DELETE /fapi/v1/batchOrders
User stream      -> POST/PUT /fapi/v1/listenKey
```

下单显式请求 `newOrderRespType=RESULT`，以保证 Order Manager 得到完整订单结果。

本地 L2 OrderBook 按 Binance 当前同步规则维护：REST Snapshot 与缓冲 Diff Depth 完成 `U/u` 首次衔接，之后要求每个事件 `pu == previous u`；发现断档立即重新拉 Snapshot，而不是继续使用可能错误的本地盘口。

项目主动交易只支持 `LIMIT/MARKET` 与 `GTC/IOC/FOK/GTX`。Binance 当前存在 `GTD/RPI`、条件单等更多枚举；User Stream 遇到不属于本系统执行面的类型会解析为 `Unsupported`，避免未知新枚举导致整条连接反序列化失败。`EXPIRED_IN_MATCH` 会归一化为本地 `Expired`。

条件单（STOP / TAKE_PROFIT / TRAILING_STOP 等）当前不属于本项目目标，因此没有为了 API 新版而引入 Algo Service。

# 运行 Runtime

主入口：`app/src/main.rs`。

```bash
cargo run -p hft-app -- <CONFIG> <SYMBOL> [SYMBOL...]
```

例如：

```bash
cargo run -p hft-app -- app/examples/local.toml BTCUSDT ETHUSDT
```

Debug：

```bash
RUST_LOG=debug cargo run -p hft-app -- app/examples/local.toml BTCUSDT
```

当前主程序用于验证行情、用户事件和 Adapter；**启动程序本身不会自动执行交易策略或自动下单**。

# 运行 Collector

Collector 与 Live 共用同一个 Binance Adapter：

```bash
mkdir -p data
cargo run -p hft-app --bin collector -- \
  app/examples/local.toml \
  data/btcusdt.csv \
  BTCUSDT
```

公开行情采集时：

```toml
api_key = ""
secret = ""
```

输出：

```text
symbol,ev,exch_ts,local_ts,px,qty,order_id,ival,fval
```

# 回测

回测目前是 `engine` 的 Rust Library 能力，没有独立 CLI。

```text
Normalized Event
  -> Replay
  -> L2 OrderBook
  -> Strategy
  -> Order Latency
  -> Queue
  -> Partial Fill
  -> Fee
  -> Position/PnL
```

默认执行模型：`PartialFillExchange`。

```bash
cargo test -p hftbacktest
cargo test -p hftbacktest --test golden_l2
```

# Release

```bash
cargo build --release --workspace
```

运行：

```bash
./target/release/hft-app app/examples/local.toml BTCUSDT
./target/release/collector app/examples/local.toml data/btcusdt.csv BTCUSDT
```

# 推荐验证顺序

```text
check / test / clippy
        ↓
Demo + Collector
        ↓
验证 L2 Snapshot / Diff 连续性
        ↓
Runtime + User Data Stream
        ↓
确定性回测
        ↓
Demo 订单生命周期
        ↓
Mainnet
```

# 项目边界

保留：

```text
Binance USD-M Futures
L2 OrderBook / Trade Tick
Queue / Latency / Partial Fill
Fee / Position / PnL
Strategy API
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

进一步文档见 `docs/`。
