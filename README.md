# HFT Backtest

面向 Binance USD-M Futures 的 Rust 订单簿高频交易项目。

## 核心边界

```text
Binance Market WS ─┐
                   ├─ Normalized Event ─ L2 OrderBook ─ Strategy
Binance User WS ───┘                         │
                                             ├─ Backtest: Queue + Latency + Partial Fill + Fee
                                             └─ Live: Binance Order Manager
```

项目不是“做市专用框架”。做市、Order Book Imbalance、Order Flow、短周期 Momentum 等策略都通过同一个 exchange-independent Strategy API 使用订单簿状态。

## 仓库

```text
engine/  纯计算：Event、Order、L2 Depth、Queue、Latency、Fill、Fee、Backtest、Strategy API
app/     I/O：Binance USD-M WS/REST、账户/订单、Collector、Live Runtime
```

明确不包含：Python、S3/AWS、L3 Market-By-Order、Bybit/Spot/Hyperliquid、Iceoryx IPC、Jupyter/ReadTheDocs。

详细设计见 `docs/`。
