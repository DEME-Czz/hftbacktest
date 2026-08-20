# Backtest

回测输入为标准化 Event 序列，按 exchange/local timestamp 推进。

核心链路：

```text
Reader -> Replay -> L2 Depth -> Strategy -> Order Latency -> Exchange Simulator -> Queue -> Partial Fill -> Fee -> Position/PnL
```

`PartialFillExchange` 是 L2AssetBuilder 默认成交模型；`NoPartialFillExchange` 仅作为显式快速近似兼容项。

`engine/tests/golden_l2.rs` 提供最小确定性 L2/Queue 基线。
