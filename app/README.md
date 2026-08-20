# App

Binance USD-M Futures I/O 与运行时。

- `hft-app`: Binance Market/User stream 运行入口，默认只消费并输出标准化事件。
- `collector`: 复用同一 Binance Adapter，把标准化 Event 写入 CSV。
- `runtime::LiveStrategyRuntime`: 维护 L2/订单/持仓状态，在完整 depth batch 后向共享 Strategy API 提供决策上下文。

实盘下单必须由调用方显式执行 StrategyCommand；默认运行入口不会自动交易。
