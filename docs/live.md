# Live

Live 只支持 Binance USD-M Futures。

Market WS 与 User WS 在同一 Rust 进程运行，不使用 Iceoryx/共享内存 IPC。

`LiveStrategyRuntime` 先更新完整 L2 depth batch，再调用共享 Strategy API。默认 CLI 不自动下单；生产调用方必须显式执行 StrategyCommand，并在执行前加入仓位、名义金额、订单频率、stale market data 和 kill-switch 风控。
