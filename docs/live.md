# Live

Live 当前只支持 Binance USD-M Futures，Market WS、User WS、Runtime 与 Strategy 都在同一个 Rust 进程内运行。

`LiveStrategyRuntime` 在完整且连续的 L2 depth batch 后调用共享 Strategy API。默认 CLI 是 dry-run，只记录通过 RiskGate 的决策；显式增加 `--execute` 后，`LiveService` 才启动账户流并允许 Submit/Cancel。

执行门禁：

- API Key/Secret 必须成对配置，远端地址必须使用 WSS/HTTPS。
- 每个 symbol 收到初始 Position 之前禁止下单。
- 私有账户流断开会清空全部 symbol 的就绪状态；重新对账前继续 fail closed。
- 风控计算当前仓位、同方向活动订单暴露、单笔数量/名义价值和活动订单数。
- Ctrl+C 只撤销本进程已跟踪的活动订单，并等待确认；两秒超时会明确告警。

当前 Live Executor 只实现 Submit/Cancel。Engine 保留 `StrategyCommand::Modify` 语义供回测使用，但 Live 收到 Modify 会拒绝并记录告警。启动时不会调用交易所级 `cancel_all`，避免误撤手工订单或其他策略订单。

## 当前生产边界

本轮验收覆盖 Demo/Testnet，不代表 Mainnet 生产就绪。进程重启后尚未通过 REST 恢复同一 `order_prefix` 的存量挂单；REST 下单结果不确定时也尚未实现按 `clientOrderId` 主动查询确认。进入 `--execute` 前必须人工确认账户没有上次进程遗留的策略订单，Mainnet 前应补齐订单恢复、stale-market 自动撤单与外部 kill switch。
