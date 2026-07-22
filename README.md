# HftBacktest 实盘、行情采集与 Alpha 训练

本项目基于 HftBacktest，提供行情采集、回测、交易所 Connector、实盘网格策略、
DeepLOB 风格的十档订单簿 Alpha 数据集、三分类模型训练和只读 TUI 监控。

所有主要程序都采用“一个程序对应一个 TOML”的配置方式。启动时只需要传入配置文件
路径，不需要在命令行重复填写币种、精度、订单数量等业务参数。

> 实盘风险：`gridtrading_live` 会向真实账户提交和撤销订单。启动生产 Connector 或
> 注册币种还可能撤销该币种已有挂单。使用前必须核对环境、账户、币种、精度、杠杆、
> 保证金、最小名义价值和持仓限制。

## 项目模块

| 目录 | 功能 | 是否具备交易能力 |
| --- | --- | --- |
| `collector` | 直接连接交易所公开 WebSocket，采集原始行情 | 否 |
| `connector` | 连接交易所行情、账户和订单接口，通过 IPC 服务策略 | 是 |
| `hftbacktest` | 回测、实盘策略、Alpha 数据采集和模型训练 | 实盘示例具备 |
| `tui` | 只读展示行情、持仓、余额、盈亏和日志 | 否 |
| `py-hftbacktest` | Python 接口 | 取决于调用方式 |

Connector、实盘策略和 TUI 使用 iceoryx2 共享内存通信，因此必须运行在同一台机器上。

## 环境要求

- Rust 工具链满足项目 `rust-version` 要求
- macOS 或 Linux
- 访问交易所 API 和 WebSocket 的网络
- 实盘功能需要对应交易所 API Key；关闭提现权限并配置 IP 白名单

检查 Rust：

```bash
rustc --version
cargo --version
```

## 构建

首次 `--release` 构建会编译和优化完整依赖树，耗时较长属于正常现象。

```bash
# 公开行情采集器
cargo build --release --package collector

# 交易所 Connector
cargo build --release --package connector

# 只读 TUI
cargo build --release --package hftbacktest-tui

# 实盘网格策略
cargo build --release --package hftbacktest --example gridtrading_live

# Alpha 模型训练
cargo build --release --package hftbacktest --example train_alpha

# 参数化回测
cargo build --release --package hftbacktest --example gridtrading_backtest_args
```

## 配置文件索引

| 程序 | 配置文件 | 主要内容 |
| --- | --- | --- |
| Collector | `collector/examples/binancefutures.toml` | 交易所、币种列表、订阅流、输出目录 |
| Binance Futures Connector | `connector/examples/binancefutures-prod.toml` | IPC 名称、接口地址、代理、订单前缀、API 凭据；不配置交易币种 |
| TUI | `tui/examples/binancefutures.toml` | IPC 名称、币种、价格精度、数量精度、刷新频率 |
| 实盘网格 | `hftbacktest/examples/gridtrading-live.toml` | 币种、精度、网格、下单量、持仓限制、Alpha 路径 |
| Alpha 训练 | `hftbacktest/examples/train-alpha.toml` | 数据集、模型输出、标签和训练参数 |
| 网格回测 | `hftbacktest/examples/gridtrading-backtest.toml` | 数据、延迟、费用、网格和输出参数 |

配置示例中的每个参数都带有用途、单位或风险说明。修改前先复制一份本地配置，尤其不要
把真实 API Key 和 Secret 提交到 Git。

## 只采集公开行情，不进行实盘交易

这是最安全的数据采集方式。`collector` 不读取账户凭据、不连接账户数据流、不注册
Connector IPC，也没有下单或撤单接口。

编辑：

```toml
# collector/examples/binancefutures.toml
output_path = "data/market"
exchange = "binancefutures"
symbols = ["dogeusdt", "btcusdt", "ethusdt"]
streams = ["$symbol@trade", "$symbol@bookTicker", "$symbol@depth@100ms"]
```

币安币种使用小写。启动：

```bash
./target/release/collector collector/examples/binancefutures.toml
```

无法直连币安时，在采集配置中设置 HTTP CONNECT 代理；直连可用时注释该项：

```toml
proxy = "127.0.0.1:7890"
```

使用 `Ctrl-C` 停止。文件按币种和 UTC 日期写入 `output_path`，重启后继续追加，不覆盖
当天已有文件。

### 原始行情与 Alpha CSV 的区别

Collector 保存交易所原始 WebSocket 消息，输出为按日轮转的 gzip 文件。它不能直接
交给 `train_alpha`。

`train_alpha` 需要同步后的十档订单簿 Alpha CSV。原始增量深度必须先经过快照对齐、
序列连续性校验和十档重建，才能转换为训练数据。不要把原始 gzip 文件直接改名为 CSV。

## 启动 Binance Futures Connector

Connector 配置文件中的 `symbols = [...]` 当前不会生效。Connector 的币种集合不是静态配置，
而是由策略或 TUI 启动后通过 IPC 的 `RegisterInstrument` 消息动态注册：

- 实盘网格的币种配置在 `hftbacktest/examples/gridtrading-live.toml` 的 `symbol`；
- TUI 的币种配置在 `tui/examples/binancefutures.toml` 的 `symbol`；
- 多币种需要启动多个策略实例，并为每个实例配置对应币种（同时确保 IPC 名称一致）。

因此，Connector 先于策略/TUI 启动时，首次账户快照日志出现 `symbols={}` 属于正常启动时序。
收到币种注册消息后会再次请求账户快照，后续日志应显示对应币种。`/fapi/v3/account` 本身是
账户级接口，请求并不携带这些 symbols。

如果账户快照返回 HTTP 418，它不是空 symbols 导致的。币安将 418 用于来源 IP 在持续触发
429 限流后的自动封禁；使用本地代理时，同一代理出口的其他程序也会共同消耗 IP 限额。
停止共享该出口的 Connector/策略进程，等待响应头 `Retry-After` 指定的封禁时间结束后再启动，
不要在封禁期间反复重启。参见[币安官方 HTTP 返回码说明](https://developers.binance.com/en/docs/products/spot/rest-api#http-return-codes)。

生产配置位于：

```text
connector/examples/binancefutures-prod.toml
```

关键字段：

```toml
name = "binancefutures-prod"
connector = "binancefutures"
market_stream_url = "wss://fstream.binance.com/ws"
user_stream_url = "wss://fstream.binance.com/private/ws"
api_url = "https://fapi.binance.com"
order_prefix = "prod"
api_key = "<BINANCE_API_KEY>"
secret = "<BINANCE_SECRET>"
```

启动：

```bash
ulimit -n 4096
./target/release/connector connector/examples/binancefutures-prod.toml
```

币安 Futures connector 默认通过 `http://127.0.0.1:7890` 代理 REST、公共 WebSocket 和私有
WebSocket。可用配置文件中的 `proxy_url` 覆盖；设置 `proxy_url = ""` 才会关闭代理并直连。

注意：

- `name` 必须与策略和 TUI 的 `connector_name` 完全一致。
- Binance Futures 币种在项目内部使用小写，例如 `dogeusdt`。
- Connector 注册或重新注册币种时可能撤销交易所上该币种的全部活动订单。
- 不要在仍需保留挂单时重启生产 Connector。
- API Key 应仅启用必要的合约交易权限，禁止提现。

## 启动只读 TUI

编辑 `tui/examples/binancefutures.toml`：

```toml
connector_name = "binancefutures-prod"
symbol = "dogeusdt"
tick_size = 0.00001
lot_size = 1.0
history_capacity = 500
poll_interval_ms = 50
```

先启动 Connector，再启动 TUI：

```bash
./target/release/hftbacktest-tui tui/examples/binancefutures.toml
```

快捷键：

- `q` 或 `Esc`：退出
- `p`：暂停或恢复界面更新

TUI 只创建接收端，不创建发单通道，不会注册币种、提交订单或撤销订单。

## 启动实盘网格策略

> 以下程序会进行真实交易。确认 Connector 环境和账户后再执行。

配置文件：

```text
hftbacktest/examples/gridtrading-live.toml
```

更换币种时不能只修改 `symbol`，必须同时核对：

- `tick_size`：交易所最小价格单位
- `lot_size`：交易所最小数量单位
- `min_grid_step`：最小网格价格间距
- `order_qty`：每张订单数量
- `max_position`：最大绝对持仓
- `dataset_path`：Alpha CSV 追加路径
- `model_path`：对应币种的模型路径

启动顺序：

```bash
# 终端 1：先启动 Connector
./target/release/connector connector/examples/binancefutures-prod.toml

# 终端 2：确认 Connector 正常后启动实盘策略
./target/release/examples/gridtrading_live \
  hftbacktest/examples/gridtrading-live.toml
```

策略启动前会等待账户状态和双边盘口初始化。超时会拒绝交易，而不是在状态不完整时继续
下单。

## Alpha 数据与模型训练

实盘策略配置中的 `dataset_path` 用于把同步后的十档订单簿记录追加到 Alpha CSV：

```toml
dataset_path = "data/doge_alpha.csv"
```

训练配置位于 `hftbacktest/examples/train-alpha.toml`：

```toml
input = "data/doge_alpha.csv"
output = "data/doge_alpha.model.json"
horizon = 50
threshold = 0.0002
train_ratio = 0.8
epochs = 100
learning_rate = 0.05
l2 = 0.0001
```

执行训练：

```bash
./target/release/examples/train_alpha \
  hftbacktest/examples/train-alpha.toml
```

训练完成后，将实盘配置中的 `model_path` 指向输出模型。模型输出上涨、横盘、下跌三分类；
低于置信度阈值的预测按横盘处理。

训练准确率不能单独证明模型可用于实盘。至少还需要检查类别分布、混淆矩阵、时间切分、
数据间断、手续费和滑点后的收益。

## 运行参数化回测

编辑：

```text
hftbacktest/examples/gridtrading-backtest.toml
```

运行：

```bash
./target/release/examples/gridtrading_backtest_args \
  hftbacktest/examples/gridtrading-backtest.toml
```

回测配置中的 `maker_fee` 和 `taker_fee` 会实际传入费用模型。负的 Maker 费率表示返佣。

## 常见日志与问题

### iceoryx2 使用默认全局配置

```text
No config file was loaded, a config with default values will be used.
origin="Config::global_config()"
```

这是 iceoryx2 IPC 没有加载专用全局配置时的警告，不是业务 TOML 加载失败。币种、交易所、
网格和 Alpha 参数仍来自本项目配置文件。没有同时出现 `NodeCreationFailure` 或 IPC 创建
失败时通常可以忽略。

### Connector 与 TUI IPC 协议不一致

如果出现 `UnexpectedVariant`，通常是 Connector、TUI 或策略使用了不同版本的
`LiveEvent`。重新构建相关二进制，并停止旧进程后再启动。

### Binance 订单过滤错误

- `-4014 Price not increased by tick size`：价格没有对齐 `tick_size`。
- `-4164 Order's notional must be no smaller than 5`：订单名义价值过小。
- `-5022 Post Only order will be rejected`：订单不能保持 Maker 身份。
- `Margin is insufficient`：可用保证金不足或订单/持仓规模过大。
- `Too many new orders`：下单频率超过交易所限制。

## 测试

```bash
# Alpha 数据、模型和追加写入测试
cargo test -p hftbacktest --test alpha

# Collector 配置测试
cargo test -p collector config::tests

# Connector 配置测试
cargo test -p connector runtime_config::tests

# TUI 配置测试
cargo test -p hftbacktest-tui config::tests
```

## 进一步文档

- `collector/README.md`：公开行情采集器
- `connector/README.md`：交易所 Connector 与生产安全说明
- `hftbacktest/README.md`：回测、实盘和训练示例
- `tui/README.md`：只读监控界面
