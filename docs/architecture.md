# Architecture

## 目标

以尽量小的运行面保留 HFT 回测与实盘所需的核心语义：L2 OrderBook、Trade Tick、Queue、Latency、Partial Fill、Fee、Position/PnL 与 StrategyCommand。

## 依赖方向

```text
app ─────────────→ engine

app/src/main.rs ──→ config ──→ live::config
        │              └──────→ exchange::binance_usdm::BinanceConfig
        └──────────→ live ────→ ports ←──── exchange::binance_usdm

app/src/bin/collector.rs ─────→ ports::MarketDataSource
```

`engine` 是同步、无网络的计算内核，不能依赖 `app`、Binance、Tokio、HTTP 或 WebSocket。`live` 只依赖应用端口，不访问 Binance 的 wire DTO；`exchange::binance_usdm` 负责实现端口。

## App 目录

```text
app/src/
├── config.rs                  # 聚合配置与启动校验
├── ports.rs                   # MarketDataSource / ExecutionVenue
├── main.rs                    # LiveService CLI
├── bin/collector.rs           # 公共行情采集 CLI
├── live/
│   ├── config.rs              # Runtime / Strategy 配置
│   ├── runtime.rs             # 标准化 L2、订单与持仓状态
│   ├── risk.rs                # 最终执行风控
│   ├── execution.rs           # StrategyCommand 执行
│   └── service.rs             # 生命周期、连接门禁与关机
└── exchange/binance_usdm/
    ├── protocol/              # Binance wire DTO/解析
    ├── market_data_stream.rs  # 公共 WS 与本地深度同步
    ├── user_data_stream.rs    # 私有 WS 与持仓同步
    ├── rest.rs                # 签名 REST
    ├── orders.rs              # REST/WS 订单回报合并
    └── transport.rs           # WebSocket、代理与退避
```

## Backtest / Live 共享边界

共享：Normalized Event、Order 语义、MarketDepth、Strategy API、StrategyCommand。

不共享：Backtest Exchange Simulator 与 Live Binance 网络实现。策略不得自行提交订单；所有 Live 命令必须经过 RiskGate 和 `--execute` 权限门禁。
