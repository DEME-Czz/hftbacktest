# Architecture

## 目标

以最少组件保留 HFT 回测真实性：L2 OrderBook、Trade Tick、Queue、Latency、Partial Fill、Fee。

## 依赖方向

```text
app -> engine
```

`engine` 不能依赖 `app`，也不能引入网络/异步运行时。

## Backtest / Live 共享边界

共享：Normalized Event、Order 语义、MarketDepth、Strategy API。

不共享：Backtest 的 Exchange Simulator 与 Live 的 Binance 网络 Runtime。
