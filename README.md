# HFT Backtest

Rust 订单簿驱动的高频交易系统，目标市场为 Binance USD-M Futures。

核心边界：

- L2 OrderBook + Trade Tick
- Queue / Latency / Partial Fill / Fee
- 确定性 Market Replay Backtest
- Rust Strategy API
- Binance USD-M Futures Live / Collector

仓库结构：

```text
engine/  # 纯 HFT 回测与微观结构计算核心
app/     # Binance I/O、Live Runtime、Collector、CLI
```

重构原则：Engine 不依赖交易所、HTTP/WebSocket、AWS 或 Python；策略不依赖 Binance 协议。
