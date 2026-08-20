# HFT Backtest

面向 **Binance USD-M Futures** 的 Rust 订单簿高频交易项目。

项目保留 HFT 回测真正需要的核心能力：L2 OrderBook、Trade Tick、Queue Position、Latency、Partial Fill、Fee、Position/PnL，并把交易所网络 I/O 与纯计算引擎分离。

## 核心架构

```text
Binance Market WS ─┐
                   ├─ Normalized Event ─ L2 OrderBook ─ Strategy
Binance User WS ───┘                         │
                                             ├─ Backtest: Queue + Latency + Partial Fill + Fee
                                             └─ Live: Binance Order Manager
```

项目不是“做市专用框架”。Market Making、Order Book Imbalance、Order Flow、短周期 Momentum 等策略都可以通过同一个 exchange-independent Strategy API 使用订单簿状态。

## 仓库结构

```text
engine/  纯计算：Event、Order、L2 Depth、Queue、Latency、Fill、Fee、Backtest、Strategy API
app/     I/O：Binance USD-M WS/REST、账户/订单、Collector、Live Runtime
```

当前明确不包含：Python、S3/AWS、L3 Market-By-Order、Bybit、Binance Spot、Hyperliquid、Iceoryx IPC、Jupyter/ReadTheDocs。

---

# 环境要求

推荐环境：

```text
Rust: 1.91.1
Cargo: 与 Rust 1.91.1 配套
OS: Linux / macOS
```

检查环境：

```bash
rustc --version
cargo --version
```

安装 Rust（未安装时）：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup toolchain install 1.91.1
rustup default 1.91.1
```

克隆并进入项目：

```bash
git clone https://github.com/DEME-Czz/hftbacktest.git
cd hftbacktest
git checkout refactor/hft-simplification
```

首次建议先执行完整检查：

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

---

# Binance 配置

项目当前只支持 **Binance USD-M Futures**。

配置示例：

```text
app/examples/binancefutures.toml
app/examples/binancefutures-demo.toml.example
```

推荐先复制一份自己的配置，不要直接修改仓库示例：

```bash
cp app/examples/binancefutures-demo.toml.example app/examples/local.toml
```

## 配置字段

```toml
# WebSocket 地址
stream_url = "wss://demo-fstream.binance.com/ws"

# REST API 地址
api_url = "https://demo-fapi.binance.com"

# 客户端订单 ID 前缀
order_prefix = "test"

# API Key
api_key = ""

# API Secret
secret = ""
```

字段说明：

| 字段 | 必须 | 说明 |
|---|---:|---|
| `stream_url` | 是 | Binance USD-M Futures WebSocket 地址 |
| `api_url` | 是 | Binance USD-M Futures REST API 地址 |
| `order_prefix` | 是 | 本地生成 client order id 时使用的前缀 |
| `api_key` | 仅私有接口需要 | 行情采集可以为空；账户、订单、下单等需要填写 |
| `secret` | 仅私有接口需要 | 请求签名使用，必须与 API Key 配套 |

## Demo 环境

建议第一次运行使用 Binance Futures Demo：

```toml
stream_url = "wss://demo-fstream.binance.com/ws"
api_url = "https://demo-fapi.binance.com"

order_prefix = "test"
api_key = "YOUR_DEMO_API_KEY"
secret = "YOUR_DEMO_SECRET"
```

如果只是采集公开行情：

```toml
api_key = ""
secret = ""
```

## Testnet 示例

仓库当前 `app/examples/binancefutures.toml` 使用：

```toml
stream_url = "wss://fstream.binancefuture.com/ws"
api_url = "https://testnet.binancefuture.com"
```

## Mainnet

正式环境：

```toml
stream_url = "wss://fstream.binance.com/ws"
api_url = "https://fapi.binance.com"
```

生产环境 API Key 建议：

- 只开启 Futures 必须权限；
- 不开启提现权限；
- 配置 IP 白名单；
- 不把真实 API Key / Secret 提交到 Git；
- 本地配置文件加入 `.gitignore`。

---

# 1. 运行 Binance Runtime

主程序入口：

```text
app/src/main.rs
```

用途：

```text
Binance Market WS
        ↓
Normalized Event
        ↓
进程内 Runtime
```

同时会启动 Binance Connector，并按照命令行传入的 symbol 注册行情。

命令格式：

```bash
cargo run -p hft-app -- <CONFIG> <SYMBOL> [SYMBOL...]
```

例如监听 BTCUSDT：

```bash
cargo run -p hft-app -- \
  app/examples/local.toml \
  BTCUSDT
```

监听多个合约：

```bash
cargo run -p hft-app -- \
  app/examples/local.toml \
  BTCUSDT ETHUSDT SOLUSDT
```

Symbol 在内部会统一转换为小写。

开启 Debug 日志：

```bash
RUST_LOG=debug cargo run -p hft-app -- \
  app/examples/local.toml \
  BTCUSDT
```

停止：

```text
Ctrl + C
```

## 当前 Runtime 行为

当前主程序主要用于验证：

- Binance WS 连接；
- 标准化 Event；
- 多 Symbol 注册；
- 用户数据 / 订单相关 Adapter 基础能力；
- 单进程 Runtime。

当前 `main.rs` **不会因为启动程序就自动运行交易策略或自动下单**。

真正策略执行应通过 `engine::strategy` 与 `app::runtime` 接入，并显式把 `StrategyCommand` 发送给 Order Manager。

因此第一次运行主程序本身不会因为订阅行情自动产生交易。

---

# 2. 运行行情 Collector

Collector 直接复用与 Live Runtime 相同的 Binance Adapter 和标准化 Event 语义，不再维护第二套行情解析器。

入口：

```text
app/src/bin/collector.rs
```

命令格式：

```bash
cargo run -p hft-app --bin collector -- <CONFIG> <OUTPUT_CSV> <SYMBOL>
```

例如采集 BTCUSDT：

```bash
mkdir -p data

cargo run -p hft-app --bin collector -- \
  app/examples/local.toml \
  data/btcusdt.csv \
  BTCUSDT
```

Collector 对公开市场数据不要求 API Key：

```toml
api_key = ""
secret = ""
```

输出 CSV：

```text
symbol,ev,exch_ts,local_ts,px,qty,order_id,ival,fval
```

字段：

| 字段 | 说明 |
|---|---|
| `symbol` | 合约代码 |
| `ev` | 标准化 Event bit flags |
| `exch_ts` | Exchange timestamp |
| `local_ts` | Local receive timestamp |
| `px` | 价格 |
| `qty` | 数量 |
| `order_id` | Event 携带的 order id；L2 行情通常不依赖该字段 |
| `ival` | 扩展整数值 |
| `fval` | 扩展浮点值 |

Collector 会持续运行，直到：

```text
Ctrl + C
```

退出时会 flush 输出文件。

---

# 3. 回测

回测核心位于：

```text
engine/
```

当前重构版把 Backtest 定义为 **Rust Library 能力**，目前没有单独提供 `backtest` CLI。

核心链路：

```text
Normalized Event
      ↓
Reader / Replay
      ↓
L2 OrderBook
      ↓
Strategy
      ↓
Order Latency
      ↓
Exchange Simulator
      ↓
Queue Model
      ↓
Partial Fill
      ↓
Fee
      ↓
Position / PnL
```

默认 L2 撮合模型：

```text
PartialFillExchange
```

`NoPartialFillExchange` 只保留为显式的快速近似模型。

## 回测验证

当前最小确定性基线：

```text
engine/tests/golden_l2.rs
```

执行：

```bash
cargo test -p hftbacktest
```

只运行 Golden L2 测试：

```bash
cargo test -p hftbacktest --test golden_l2
```

完整 workspace 测试：

```bash
cargo test --workspace
```

## 开发自己的回测策略

策略应基于：

```text
engine/src/strategy.rs
```

核心接口负责把：

```text
MarketContext
      ↓
Strategy
      ↓
StrategyCommand
```

与 Binance 解耦。

也就是说 Strategy 不应该直接处理：

```text
Binance JSON
WebSocket Message
REST Response
```

而应该只使用标准化市场状态。

后续如果需要增加独立回测 CLI，应该放在：

```text
app/src/bin/backtest.rs
```

而不是把 CLI / 配置读取塞进 `engine`。

---

# 4. 开发模式

格式化：

```bash
cargo fmt --all
```

编译检查：

```bash
cargo check --workspace --all-targets
```

测试：

```bash
cargo test --workspace
```

Clippy：

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

推荐提交前完整执行：

```bash
cargo fmt --all && \
cargo check --workspace --all-targets && \
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings
```

当前 CI 使用 Rust `1.91.1` 执行同样的检查。

---

# 5. Release 构建

构建全部 release binary：

```bash
cargo build --release --workspace
```

构建 app：

```bash
cargo build --release -p hft-app
```

构建完成后主要 binary 位于：

```text
target/release/hft-app
target/release/collector
```

直接运行 Runtime：

```bash
./target/release/hft-app \
  app/examples/local.toml \
  BTCUSDT
```

运行 Collector：

```bash
./target/release/collector \
  app/examples/local.toml \
  data/btcusdt.csv \
  BTCUSDT
```

---

# 6. 推荐运行顺序

第一次使用项目建议严格按照以下顺序：

```text
1. cargo check / test / clippy
        ↓
2. 配置 Binance Demo
        ↓
3. 启动 Collector 验证 Market WS
        ↓
4. 检查 CSV 标准化 Event
        ↓
5. 启动 hft-app 验证单进程 Runtime
        ↓
6. 开发 Strategy
        ↓
7. 使用 engine 做确定性回测
        ↓
8. Binance Demo 验证订单生命周期
        ↓
9. 最后才考虑 Mainnet
```

不要直接从 Mainnet 自动下单开始验证系统。

---

# 7. 当前边界

当前版本刻意只保留满足目标所必需的组件：

```text
Binance USD-M Futures
L2 OrderBook
Trade Tick
Queue
Latency
Partial Fill
Fee
Position / PnL
Strategy API
Collector
Live Adapter
```

当前不计划因为“以后可能会用”重新加入：

```text
Python Binding
AWS/S3
L3 Market-By-Order
多交易所 Connector
Iceoryx IPC
多进程 Connector
```

如果未来真的出现第二交易所或多进程隔离的明确需求，再从实际约束重新设计。

---

# 文档

进一步设计说明：

```text
docs/architecture.md
docs/backtest.md
docs/live.md
docs/data.md
```
