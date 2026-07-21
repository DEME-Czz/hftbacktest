# HftBacktest TUI 展示方案

## 1. 定位

`tui` 用于在终端中观察现有实盘链路，第一阶段只做只读监控，不提交、撤销或修改订单。

目标是让操作者在一个界面中看到：

- Connector、策略和 IPC 是否仍有数据活动
- 当前交易品种的盘口、最新成交和行情延迟
- 当前持仓、活动订单、成交回报和订单延迟
- Connector 上报的连接异常与订单错误

TUI 不是交易账户管理工具。当前 IPC 会发布交易品种报价资产的钱包余额和本次 TUI 运行期间收到的成交/手续费；可用保证金、保证金率、杠杆和完整账户状态仍未发布。

当前 Overview 还会直接从已有事件计算并展示：

- `LONG`、`SHORT`、`FLAT` 或尚未初始化的 `WAITING` 持仓方向
- 中间价、价差及基点（bp）、按中间价估算的仓位名义价值
- 活动订单总数，以及买单和卖单数量
- 钱包余额相对 TUI 首次收到余额时的变化（明确标记为 `not PnL`）
- 持仓和账户数据距今时长
- 连续重复错误的聚合次数，避免相同错误刷屏

由于当前 `LiveEvent::Fill` 没有成交方向，`LiveEvent::Position` 也没有入场价、标记价和交易所盈亏字段，TUI 不会从成交量猜测盈利。已实现和未实现盈亏会显示为 `unavailable (protocol)`。

## 2. 启动方式

当前目录已经提供独立二进制 `hftbacktest-tui`。先构建：

```bash
cargo build --release --package hftbacktest-tui
```

生产 Connector 启动后运行：

```bash
./target/release/hftbacktest-tui \
    binancefutures-prod \
    --symbol dogeusdt \
    --tick-size 0.00001 \
    --lot-size 1
```

其中：

- `binancefutures-prod` 是已运行 Connector 的 IPC `NAME`，必须与 Connector 启动参数一致
- `symbol` 使用 Connector 要求的大小写；Binance 为小写，Bybit 为大写
- TUI 应在 Connector 之后启动；策略可以在 TUI 之前或之后启动
- 第一阶段不读取交易所配置文件，也不接触 API Key 或 Secret

## 3. 主界面

默认使用单屏仪表盘，适配至少 `120 x 32` 的终端：

```text
┌ HftBacktest Live ─ binancefutures-prod ─ DOGEUSDT ─ READ ONLY ─ 09:17:00 ┐
│ IPC ● ACTIVE  Feed age 12ms  Order age 318ms  Errors 2  Uptime 00:41:23       │
├ Market ───────────────────────────────┬ Position ─────────────────────────────┤
│ Last       0.18342                    │ Net qty                   +300 DOGE    │
│ Best ask   0.18343  qty 8,241         │ Mark/entry price          unavailable │
│ Spread     0.00001  (0.55 bp)         │ Wallet / margin / PnL     unavailable │
│ Best bid   0.18342  qty 5,907         │ Source                    Position evt │
│ Feed lag   exch→local 12ms             │ Updated                   09:16:59.982 │
├ Order Book ───────────────────────────┼ Working Orders ───────────────────────┤
│ ASK  0.18347  ██████       18,201     │ ID       Side Price    Qty  Left State │
│ ASK  0.18346  ███          10,022     │ 1042     SELL 0.18400  100  100  New   │
│ ASK  0.18345  █████        15,530     │ 1041     BUY  0.18290  100  100  New   │
│ ASK  0.18344  ██            7,310     │                                      │
│ ASK  0.18343  ██            8,241     │                                      │
│ BID  0.18342  ██            5,907     │                                      │
│ BID  0.18341  █████        14,774     │                                      │
│ BID  0.18340  ████         12,606     │                                      │
├ Recent Trades ────────────────────────┼ Events / Errors ──────────────────────┤
│ 09:16:59.910 BUY  0.18342  2,104      │ 09:17:00 ERROR -4014 invalid tick     │
│ 09:16:59.862 SELL 0.18341    230      │ 09:16:58 ORDER 1042 accepted          │
│ 09:16:59.801 BUY  0.18342  1,091      │ 09:16:55 POSITION +200 → +300         │
└ [1]Overview [2]Orders [3]Latency [4]Events  [Tab]Symbol  [p]Pause  [q]Quit ───┘
```

颜色约定：绿色表示买入、正常或延迟较低；红色表示卖出、错误或数据中断；黄色表示警告或数据变旧；灰色 `unavailable` 表示现有代码没有提供该数据，不能用 `0` 冒充。

## 4. 页面设计

### Overview

默认首页，展示 BBO、价差、五到十档盘口、最近成交、净持仓、活动订单和最近事件。窗口较小时按优先级隐藏最近成交、事件列表和深档盘口，但始终保留连接状态、BBO、持仓与活动订单。

### Orders

订单表支持按状态过滤：`Active`、`Filled`、`Canceled`、`Rejected`、`All`。字段来自现有 `Order`：

| 字段 | 数据来源 |
| --- | --- |
| ID | `order_id` |
| Side | `side` |
| Price | `price_tick * tick_size` |
| Qty / Left / Filled | `qty` / `leaves_qty` / `exec_qty` |
| Status / Request | `status` / `req` |
| Type / TIF | `order_type` / `time_in_force` |
| Exchange time | `exch_timestamp` |
| Local time | `local_timestamp` |
| Maker | `maker` |

第一阶段不提供撤单快捷键。后续若增加交易操作，必须单独启用 `--trading-enabled`，并为撤单和批量撤单提供明确的二次确认。

### Latency

显示滚动窗口内的：

- 行情延迟：`event.local_ts - event.exch_ts`
- 订单请求延迟：`exch_timestamp - local_timestamp`
- 订单往返延迟：`receive_timestamp - local_timestamp`
- 最近 1 分钟事件速率、最大值、P50、P95、P99
- 最后一次行情、订单、持仓和错误事件距今时长

延迟统计只在内存中保留固定长度环形缓冲区，避免 TUI 长时间运行后持续增长。

### Events

按时间倒序展示 `Feed`、`Order`、`Position` 和 `Error`，支持按类型、品种和错误级别过滤。错误详情完整显示 `LiveError.kind` 和 `LiveError.value`，例如 Binance 的 `-4014` 与 `-4164`。

## 5. 与现有代码的对应关系

```text
Exchange
   │
   ▼
Connector ── Iceoryx <NAME>/ToBot ──► Trading Bot
                         └───────────► TUI（只订阅广播事件）
```

第一阶段 TUI 直接用 `IceoryxBuilder::new(name).receiver::<LiveEvent>()` 订阅 `<NAME>/ToBot`，自行维护展示状态：

- `LiveEvent::Feed` 更新盘口、BBO、最近成交和行情延迟
- `LiveEvent::Order` 更新订单表、成交记录和订单延迟
- `LiveEvent::Position` 更新净持仓
- `LiveEvent::Balance` 更新该品种报价资产的钱包余额
- `LiveEvent::Fill` 更新成交次数、成交量和手续费累计值
- `LiveEvent::Error` 写入告警列表
- `BatchStart` / `BatchEnd` 用于原子刷新快照

TUI 不应使用 `LiveBotBuilder`，也不应向 `<NAME>/FromBot` 发送 `RegisterInstrument`。当前 Connector 在注册品种时可能撤销该品种的全部活动订单；监控工具不能触发这一副作用。

这个只读旁路方案有一个边界：TUI 只能收到启动之后的广播事件，无法主动索取初始快照。推荐在第二阶段为协议新增无交易副作用的 `SubscribeTelemetry` 请求，或者由 Connector 增加独立的只读状态/遥测通道，让后启动的 TUI 能安全取得盘口、订单和持仓快照。
因此 TUI 启动时显示 `unavailable` 不代表交易所一定没有该数据；它可能只是在等待下一条
行情、余额或持仓更新。交易策略通过 `RegisterInstrument` 主动取得快照，不受这个只读限制。

## 6. 状态与告警规则

TUI 不能仅凭“进程存在”判断真实连接状态，第一阶段按事件新鲜度推断：

| 状态 | 默认规则 |
| --- | --- |
| `ACTIVE` | 最近行情事件不超过 2 秒 |
| `STALE` | 2 秒到 10 秒没有行情事件 |
| `DISCONNECTED` | 超过 10 秒无行情，或收到 `ConnectionInterrupted` |
| `CRITICAL` | 收到 `CriticalConnectionError` |

阈值应允许通过 CLI 配置。低成交量品种不能用“最近成交时间”判断连接，必须使用盘口或其他 Feed 事件。

以下内容在当前 IPC 中不可用，界面必须明确标为 `unavailable`：

- 可用余额和可用保证金
- 未实现盈亏、已实现盈亏、保证金率
- 仓位入场价、标记价格、强平价和杠杆
- Connector WebSocket/REST 的精确连接状态
- 策略内部参数、循环状态和主动停止原因

## 7. 当前目录结构

`tui` 已作为独立 workspace crate 接入：

```text
tui/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs           # 可测试的状态模型导出
    ├── main.rs          # CLI、Iceoryx 只读订阅、事件循环和终端恢复
    ├── model.rs         # 盘口、订单、持仓、延迟和健康状态聚合
    └── ui.rs            # Ratatui 页面布局和响应式降级
└── tests/
    └── model.rs         # 状态聚合与健康阈值测试
```

当前实现使用 `ratatui`、`crossterm`、`clap`，并依赖 workspace 内的 `hftbacktest` 复用 `LiveEvent`、`Order` 和 Iceoryx 类型。

## 8. 实施顺序

1. 已完成 MVP：单 Connector、单品种、只读 Overview；支持盘口、最近成交、持仓、订单、事件、延迟、健康状态、暂停刷新与安全退出。
2. 后续可观测性：独立的 Orders、Latency、Events 页面，以及 P50/P95/P99 延迟统计。
3. 多品种：CLI 接收多个 symbol，Tab 切换并增加总览列表。
4. 协议增强：无副作用的只读快照与明确的 Connector 健康事件。
5. 账户遥测：Connector 发布余额、保证金、PnL 等账户字段后再增加账户面板。
6. 可选交易控制：仅在单独安全评审后实现，默认关闭，并增加权限开关、确认流程与审计日志。

## 9. MVP 验收标准

- TUI 启停不发送任何 `LiveRequest`，不影响 Connector 和策略运行
- 能正确展示 DOGEUSDT 的 BBO、五档盘口、净持仓和订单状态变化
- `-4014`、`-4164` 等订单错误能在界面中保留和查看
- 终端缩放不会 panic，退出后光标和终端模式正常恢复
- IPC 暂时无数据时界面保持响应，并显示 `STALE` 或 `DISCONNECTED`
- 不读取、记录或显示 API Key、Secret 与签名请求内容
- 能显示报价资产钱包余额，以及 TUI 启动后收到的成交量和手续费累计值
- 对当前不存在的数据明确显示 `unavailable`
