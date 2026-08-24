# App

`app` 提供 Binance USD-M Futures I/O、实时状态管理与策略执行入口。

- `config.rs`：组合并校验 Binance、Runtime、策略和风险配置；错误不会回显凭证。
- `ports.rs`：`MarketDataSource` 与 `ExecutionVenue` 两个应用端口。
- `exchange/binance_usdm/`：WebSocket、REST、协议 DTO 和订单回报合并。
- `live/`：L2/订单/持仓状态、RiskGate、Executor 与 `LiveService`。
- `bin/collector.rs`：只启动公共行情，把标准化 Event 写入 CSV。

`hft-app` 默认是 decision-only dry-run；只有显式传入 `--execute`，完成凭证/端点校验并收到初始持仓后才允许 Submit/Cancel。账户流断开会立即暂停执行，逐个 symbol 重新完成持仓对账后才恢复。
