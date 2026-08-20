# hftbacktest 基于论文三分类的方向感知网格完整落地方案

> 目标：在 `nkaz001/hftbacktest` 的现有网格策略上增加上涨、震荡、下跌识别，并在确认的单边行情中暂停逆势开仓网格，同时保留减仓能力，防止趋势行情持续累积逆势仓位。

---

## 1. 方案边界

本方案由两部分组成：

1. **论文预测层**
   - 三分类：上涨、震荡、下跌；
   - 多项 Logit；
   - 按特征组进行 Group LASSO 正则化；
   - 输出 `P(up)`、`P(sideways)`、`P(down)`。

2. **交易工程层**
   - 概率滞回状态机；
   - 开仓方向许可；
   - 开仓和减仓订单意图区分；
   - 持仓非零时禁止直接反向开仓；
   - 模型失效、行情异常时进入 `RiskOff`；
   - 回测、影子运行、模拟盘和小额实盘验收。

论文负责回答“未来更可能属于哪个方向”，交易工程层负责回答“当前允许挂哪些订单”。

---

## 2. 最终目标与不可改变的约束

### 2.1 最终目标

在不替换 `hftbacktest` 撮合、延迟、队列和行情回放能力的前提下，实现：

```text
上涨状态：
  允许开多
  禁止新开空
  允许平多
  允许平空

下跌状态：
  允许开空
  禁止新开多
  允许平多
  允许平空

震荡状态：
  允许双向网格

不确定状态：
  禁止新开仓
  仅允许减仓

风险状态：
  撤销全部开仓单
  主动降低或清空仓位
```

### 2.2 客观约束

1. 市场状态无法被完全正确预测。
2. 撤单存在延迟，状态切换后旧订单仍可能成交。
3. `Buy` 不一定是开多，也可能是平空。
4. `Sell` 不一定是开空，也可能是平多。
5. 单纯移动报价中心不能阻止逆势开仓。
6. 训练特征和实盘特征必须使用完全相同的计算逻辑。
7. 高频特征不能依赖未来数据、未闭合窗口或回填数据。
8. 模型不能直接进入100毫秒订单热路径执行复杂训练或动态分配。
9. 实盘必须有交易所 `reduceOnly` 和策略层数量预算两道保护。

---

## 3. 第一性原则下的最小系统

为完成目标，最少只需要以下组件：

```text
L2深度和逐笔成交
        ↓
统一FeatureEngine
        ↓
离线数据集与标签
        ↓
G-LASSO多项Logit训练
        ↓
模型JSON
        ↓
Rust实时推理
        ↓
Regime状态机
        ↓
DirectionPolicy
        ↓
GridPlanner
        ↓
OrderReconciler
```

MVP不实现：

- 深度神经网络；
- 在线训练；
- 多模型投票；
- 强化学习；
- 自动超参数无限搜索；
- 新闻和情绪数据；
- G-SCAD和G-MCP。

原因不是这些方法无效，而是它们不是实现“禁止逆势网格”所必需的组件。

---

## 4. 当前项目问题定位

现有示例策略的关键问题是：

```rust
let alpha = 0.0;
let forecast_mid_price = mid_price + alpha;
```

随后分别生成完整买入网格和卖出网格。持仓只改变两侧报价距离，并通过 `max_position` 限制仓位，但没有“当前方向是否允许新开仓”的判断。

因此，即使加入趋势预测，只修改 `alpha` 仍然不能解决：

```text
上涨中持续挂Sell网格 → 多仓平完后继续成交 → 反向开空
下跌中持续挂Buy网格 → 空仓平完后继续成交 → 反向开多
```

必须把订单拆分成“开仓意图”和“减仓意图”。

---

## 5. 论文模型的工程化定义

## 5.1 标签

论文使用未来收益与阈值 `c` 将行情划分为三类。本方案保留这个结构，但将固定阈值改成适合高频网格的动态阈值。

定义未来 `H` 秒中间价对数收益：

\[
r_{t,H}=\log\left(\frac{mid_{t+H}}{mid_t}\right)
\]

动态阈值：

\[
c_t=\max(c_{cost},\ k_\sigma \sigma_{t,H},\ k_g g_t)
\]

其中：

- `c_cost`：手续费、滑点和逆向选择构成的最小有效波动；
- `σ(t,H)`：当前滚动波动率换算到预测周期；
- `g(t)`：当前网格间距相对中间价的比例；
- `kσ`、`kg`：离线校准参数。

标签：

\[
Y_t=
\begin{cases}
UP, & r_{t,H}>c_t \\
SIDEWAYS, & |r_{t,H}|\le c_t \\
DOWN, & r_{t,H}<-c_t
\end{cases}
\]

### MVP默认值

```yaml
sample_interval_ms: 1000
prediction_horizon_ms: 60000
label:
  volatility_multiplier: 0.80
  grid_multiplier: 2.00
  cost_floor_bps: 2.00
```

这些只是初始值，最终必须由样本外回测选择。

---

## 5.2 特征组

论文的关键思想不是某个固定指标，而是将相关指标按组正则化。本方案使用五组、共24个高频特征。

所有价格类特征使用中间价：

\[
mid_t=\frac{bestBid_t+bestAsk_t}{2}
\]

### G1：价格趋势组，6个

| 特征 | 公式或定义 |
|---|---|
| `ret_1s` | `log(mid_t / mid_t-1s)` |
| `ret_5s` | `log(mid_t / mid_t-5s)` |
| `ret_15s` | `log(mid_t / mid_t-15s)` |
| `ret_60s` | `log(mid_t / mid_t-60s)` |
| `ema_spread_10_60` | `(EMA10s-EMA60s)/mid` |
| `efficiency_60s` | `abs(mid_t-mid_t-60s)/sum(abs(delta_mid))` |

### G2：波动和区间组，5个

| 特征 | 定义 |
|---|---|
| `rv_5s` | 5秒实现波动率 |
| `rv_30s` | 30秒实现波动率 |
| `rv_300s` | 300秒实现波动率 |
| `vol_ratio_30_300` | `rv_30s/(rv_300s+eps)` |
| `range_60s_bps` | 60秒最高中间价和最低中间价差 |

### G3：订单簿组，5个

订单簿不平衡：

\[
I_n=\frac{\sum_{i=1}^n bidQty_i-\sum_{i=1}^n askQty_i}
{\sum_{i=1}^n bidQty_i+\sum_{i=1}^n askQty_i+\epsilon}
\]

| 特征 | 定义 |
|---|---|
| `spread_bps` | `(ask-bid)/mid*10000` |
| `imbalance_l1` | 一档深度不平衡 |
| `imbalance_l5` | 前五档深度不平衡 |
| `imbalance_l10` | 前十档深度不平衡 |
| `microprice_delta_bps` | `(microprice-mid)/mid*10000` |

Microprice：

\[
microprice=
\frac{ask\cdot bidQty_1+bid\cdot askQty_1}
{bidQty_1+askQty_1}
\]

### G4：成交流组，5个

主动成交不平衡：

\[
TI_w=\frac{buyVolume_w-sellVolume_w}
{buyVolume_w+sellVolume_w+\epsilon}
\]

| 特征 | 定义 |
|---|---|
| `trade_imbalance_1s` | 1秒主动成交不平衡 |
| `trade_imbalance_5s` | 5秒主动成交不平衡 |
| `trade_imbalance_15s` | 15秒主动成交不平衡 |
| `cvd_slope_15s` | 15秒累计成交量差斜率 |
| `trade_intensity_ratio` | 5秒成交笔数/60秒均值 |

### G5：流动性变化组，3个

| 特征 | 定义 |
|---|---|
| `depth_l5_log` | `log(1 + bidDepth5 + askDepth5)` |
| `depth_change_5s` | 五档总深度5秒变化率 |
| `ofi_5s` | 五秒订单流不平衡 |

总特征数：

```text
6 + 5 + 5 + 5 + 3 = 24
```

---

## 5.3 特征计算规则

1. 以交易所时间排序。
2. 每100毫秒处理行情事件。
3. 每1秒生成一个模型样本。
4. 特征只能使用 `t` 时刻及以前数据。
5. 最大窗口为300秒，因此模型启动预热时间至少300秒。
6. 逐笔成交处理完成后调用：

```rust
hbt.clear_last_trades(Some(0));
```

防止同一成交被重复累计。

7. 缺失条件：
   - 深度不完整：当前推理无效；
   - 连续缺失超过2秒：进入 `RiskOff`；
   - 成交流缺失但深度正常：允许推理，但写入数据质量标志；
   - 价格为0、NaN或spread异常：禁止推理。

8. 所有标准化参数只能由训练集计算：

\[
x'_j=clip\left(\frac{x_j-\mu_j}{s_j},-8,8\right)
\]

---

## 5.4 多项Logit

为减少三类参数冗余，将 `SIDEWAYS` 设置为参考类：

\[
\eta_{up}=b_{up}+x^\top\beta_{up}
\]

\[
\eta_{down}=b_{down}+x^\top\beta_{down}
\]

\[
D=1+\exp(\eta_{up})+\exp(\eta_{down})
\]

\[
P(up)=\frac{\exp(\eta_{up})}{D}
\]

\[
P(sideways)=\frac{1}{D}
\]

\[
P(down)=\frac{\exp(\eta_{down})}{D}
\]

该形式和三类Softmax等价，但参数更少、推理更稳定。

---

## 5.5 Group LASSO目标函数

MVP只实现凸优化的G-LASSO：

\[
\min_B
-\frac{1}{N}\sum_{i=1}^{N}\log P(y_i|x_i)
+\lambda\sum_{k\in\{up,down\}}\sum_{g=1}^{G}
\sqrt{d_g}\|\beta_{k,g}\|_2
\]

其中：

- `g` 是特征组；
- `dg` 是该组特征数量；
- `λ` 控制整组特征是否被压缩为0。

### 为什么MVP不用SCAD/MCP

- G-LASSO是凸优化，结果可复现；
- 便于使用近端梯度/FISTA实现；
- 模型规模只有24个特征，训练速度不是瓶颈；
- 当前目标是验证方向过滤能否降低逆势库存和回撤；
- SCAD/MCP只在G-LASSO明显欠拟合后增加。

---

## 5.6 训练算法

采用FISTA近端梯度。

普通梯度步骤：

```text
Z = B - learning_rate * gradient(loss)
```

每个特征组应用Group Soft Threshold：

\[
B_g =
\left(
1-\frac{\alpha\lambda\sqrt{d_g}}
{\|Z_g\|_2}
\right)_+ Z_g
\]

停止条件：

```yaml
max_iterations: 5000
relative_tolerance: 1.0e-7
gradient_backtracking: true
```

### 类别权重

为避免震荡样本占比过高：

\[
w_k=\frac{1}{\sqrt{frequency_k}}
\]

再将权重裁剪到：

```text
[0.5, 2.0]
```

### 错误成本

`UP` 被预测成 `DOWN`，以及 `DOWN` 被预测成 `UP`，对交易最危险。

验证集模型选择使用成本矩阵：

| 真实\预测 | UP | SIDEWAYS | DOWN |
|---|---:|---:|---:|
| UP | 0 | 1 | 3 |
| SIDEWAYS | 1 | 0 | 1 |
| DOWN | 3 | 1 | 0 |

---

## 5.7 时间序列验证

禁止随机打乱数据。

采用Purged Walk-Forward：

```text
Fold 1: Train A → Validate B → Test C
Fold 2: Train A+B → Validate C → Test D
Fold 3: Train A+B+C → Validate D → Test E
```

训练集和验证集之间清除：

```text
purge = prediction_horizon + max_feature_window
```

例如：

```text
60秒预测周期 + 300秒最大窗口 = 360秒
```

最终测试集只能使用一次，不能参与阈值选择。

### 选择标准

第一层：模型有效性

- 成本加权Log Loss；
- Macro F1；
- `UP↔DOWN`直接误判率；
- Kappa；
- 概率校准误差。

第二层：策略有效性

- 单边区间逆势新增仓位；
- 最大库存；
- 最大回撤；
- 成交后1秒、5秒、30秒markout；
- 净收益和手续费；
- 撤单次数。

---

## 5.8 概率校准

状态机依赖概率阈值，因此训练后必须在验证集做温度缩放：

\[
P_k=\text{softmax}(z_k/T)
\]

导出一个标量 `temperature`。

禁止直接使用训练集选择温度。

---

## 5.9 模型导出

模型文件：

```text
models/regime_glasso_v001.json
```

结构：

```json
{
  "model_type": "multinomial_group_lasso",
  "version": "regime_glasso_v001",
  "sample_interval_ms": 1000,
  "prediction_horizon_ms": 60000,
  "feature_schema_hash": "sha256:...",
  "features": ["ret_1s", "ret_5s"],
  "groups": {
    "trend": [0, 1, 2, 3, 4, 5]
  },
  "mean": [0.0],
  "std": [1.0],
  "intercept_up": 0.0,
  "intercept_down": 0.0,
  "coef_up": [0.0],
  "coef_down": [0.0],
  "temperature": 1.0,
  "training_start": "YYYY-MM-DD",
  "training_end": "YYYY-MM-DD"
}
```

Rust启动时必须校验：

- 特征数量；
- 特征顺序；
- schema hash；
- 标准差非0；
- 系数无NaN；
- 模型版本非空；
- 预测周期和配置一致。

校验失败直接进入 `RiskOff`，禁止使用默认全0模型继续交易。

---

## 6. 实时状态机

## 6.1 状态定义

MVP只保留五个状态：

```rust
pub enum Regime {
    Up,
    Sideways,
    Down,
    Uncertain,
    RiskOff,
}
```

不使用 `UpWeak/UpStrong`，避免不必要的状态组合。趋势强度直接映射为仓位比例。

---

## 6.2 状态进入条件

定义：

```text
p_max  = max(p_up, p_sideways, p_down)
margin = p_max - second_max_probability
```

默认参数：

```yaml
regime:
  enter_probability: 0.62
  enter_margin: 0.15
  exit_probability: 0.50
  confirmation_count: 3
  minimum_hold_ms: 5000
  stale_after_ms: 3000
```

进入 `UP`：

```text
p_up >= 0.62
p_up - max(p_sideways, p_down) >= 0.15
连续3次成立
```

进入 `DOWN` 同理。

进入 `SIDEWAYS`：

```text
p_sideways >= 0.58
p_sideways - max(p_up, p_down) >= 0.10
连续3次成立
```

否则进入或保持 `UNCERTAIN`。

### 滞回规则

1. `UP` 不允许直接跳到 `DOWN`。
2. `DOWN` 不允许直接跳到 `UP`。
3. 相反方向出现时，先进入 `UNCERTAIN`。
4. 状态至少保持5秒，除非触发风险条件。
5. 当前状态概率低于退出阈值时，进入 `UNCERTAIN`。

---

## 6.3 RiskOff条件

任一条件成立立即进入 `RiskOff`：

```text
模型文件无效
模型推理超过3秒未更新
深度超过2秒未更新
best_bid >= best_ask
spread超过配置上限
短周期波动率超过历史分位上限
持仓超过硬限制
当日亏损超过限制
连接器或账户状态不可信
订单状态长时间无法对账
```

`RiskOff` 动作：

1. 取消全部开仓订单；
2. 保留或重新创建reduce-only减仓订单；
3. 根据配置选择被动减仓或主动清仓；
4. 状态恢复后仍需连续确认，不能直接恢复开仓。

---

## 7. 方向策略

## 7.1 订单意图

```rust
pub enum OrderIntent {
    OpenLong,
    ReduceShort,
    OpenShort,
    ReduceLong,
}
```

每个策略订单必须记录：

```rust
pub struct StrategyOrderMeta {
    pub intent: OrderIntent,
    pub created_regime: Regime,
    pub regime_version: u64,
    pub price_tick: i64,
    pub qty: f64,
}
```

订单ID不能继续只使用价格tick，否则无法区分意图。

建议编码：

```text
高3位：OrderIntent
第60位：Side
低60位：price_tick
```

---

## 7.2 基础许可矩阵

| Regime | OpenLong | OpenShort | ReduceLong | ReduceShort |
|---|---:|---:|---:|---:|
| Up | 允许 | 禁止 | 允许 | 允许 |
| Sideways | 允许 | 允许 | 允许 | 允许 |
| Down | 禁止 | 允许 | 允许 | 允许 |
| Uncertain | 禁止 | 禁止 | 允许 | 允许 |
| RiskOff | 禁止 | 禁止 | 强制 | 强制 |

---

## 7.3 禁止直接穿仓反向

这是整个方案最重要的订单约束。

### 当前持有多仓

```text
允许：
  OpenLong
  ReduceLong

禁止：
  OpenShort
```

即使状态已经从上涨切换为下跌，也必须：

```text
先ReduceLong到0
确认仓位为0
确认ReduceLong订单全部终结
下一轮才允许OpenShort
```

### 当前持有空仓

同理：

```text
先ReduceShort到0
确认仓位为0
下一轮才允许OpenLong
```

这样即使没有交易所reduce-only，也不会因为一组卖单或买单持续成交而直接翻转持仓。

---

## 7.4 不同状态和持仓下的订单

### UP

| 当前仓位 | Buy订单 | Sell订单 |
|---|---|---|
| `position > 0` | OpenLong | ReduceLong |
| `position = 0` | OpenLong | 无 |
| `position < 0` | ReduceShort | 无 |

### DOWN

| 当前仓位 | Buy订单 | Sell订单 |
|---|---|---|
| `position > 0` | 无 | ReduceLong |
| `position = 0` | 无 | OpenShort |
| `position < 0` | ReduceShort | OpenShort |

### SIDEWAYS

| 当前仓位 | Buy订单 | Sell订单 |
|---|---|---|
| `position > 0` | OpenLong | ReduceLong |
| `position = 0` | OpenLong | OpenShort |
| `position < 0` | ReduceShort | OpenShort |

### UNCERTAIN

| 当前仓位 | Buy订单 | Sell订单 |
|---|---|---|
| `position > 0` | 无 | ReduceLong |
| `position = 0` | 无 | 无 |
| `position < 0` | ReduceShort | 无 |

此设计刻意不在持仓非零时同时挂反向开仓单。

---

## 7.5 数量预算

定义：

```rust
long_position  = position.max(0.0);
short_position = (-position).max(0.0);
```

未成交订单按意图累计：

```rust
pending_open_long
pending_reduce_short
pending_open_short
pending_reduce_long
```

预算：

```rust
open_long_budget =
    max_long_position - long_position - pending_open_long;

open_short_budget =
    max_short_position - short_position - pending_open_short;

reduce_long_budget =
    long_position - pending_reduce_long;

reduce_short_budget =
    short_position - pending_reduce_short;
```

全部使用：

```rust
budget.max(0.0)
```

硬约束：

```text
pending_reduce_long  <= long_position
pending_reduce_short <= short_position
```

若订单部分成交，下一次循环立即重算预算。

---

## 8. 网格生成

## 8.1 网格中心

方向概率边际：

\[
edge=P(up)-P(down)
\]

预测中心偏移：

\[
\alpha =
clip(k_\alpha \cdot edge \cdot \sigma_H,
-\alpha_{max},
\alpha_{max})
\]

\[
forecastMid=mid\cdot e^\alpha
\]

MVP限制：

```text
abs(alpha) <= 1个网格间距
```

方向安全由许可矩阵控制，`alpha` 只负责小幅调整报价中心。

---

## 8.2 目标仓位

\[
targetPosition =
maxPosition \cdot clip\left(\frac{edge}{edgeFull},-1,1\right)
\]

默认：

```yaml
position:
  edge_full: 0.50
```

库存偏斜从：

```rust
normalized_position = position / order_qty;
```

改为：

```rust
inventory_error = position - target_position;
normalized_position = inventory_error / order_qty;
```

这样报价围绕目标仓位而不是绝对零仓位调整。

---

## 8.3 趋势仓位缩放

```text
confidence = clamp((p_max - enter_probability) /
                   (1 - enter_probability), 0, 1)
```

趋势状态最大开仓：

```text
directional_limit =
max_position * (0.5 + 0.5 * confidence)
```

即刚确认趋势时只允许50%最大仓位，概率越高才逐步放大。

---

## 8.4 网格层数

MVP：

```yaml
grid:
  open_levels_sideways: 10
  open_levels_trend: 6
  reduce_levels: 4
```

趋势中减少开仓层数，避免单边加仓过快。

减仓网格应比开仓网格更靠近市场：

```text
reduce_half_spread <= open_half_spread
```

---

## 9. 运行循环

核心伪代码：

```rust
while hbt.elapse(100_000_000)? == ElapseResult::Ok {
    let now = hbt.current_timestamp();

    feature_engine.on_depth(hbt.depth(0), now);
    feature_engine.on_trades(hbt.last_trades(0), now);
    hbt.clear_last_trades(Some(0));

    if feature_engine.ready_for_sample(now) {
        match model.predict(feature_engine.snapshot()) {
            Ok(prediction) => regime_machine.update(prediction, now),
            Err(_) => regime_machine.enter_risk_off(now),
        }
    }

    let position = hbt.position(0);
    let policy = direction_policy.resolve(
        regime_machine.current(),
        position,
        regime_machine.last_prediction(),
    );

    let current_orders = order_registry.snapshot(hbt.orders(0));

    // 先撤销新状态下不再允许的开仓单
    let forbidden = reconciler.find_forbidden_orders(
        &current_orders,
        &policy,
        regime_machine.version(),
    );
    cancel_orders(hbt, forbidden)?;

    // 撤单未确认前继续计入预算
    let budgets = risk_budget.calculate(
        position,
        hbt.orders(0),
        &order_registry,
        &policy,
    );

    // 持仓非零时禁止直接反向开仓
    let desired_orders = grid_planner.plan(
        hbt.depth(0),
        position,
        &budgets,
        &policy,
        regime_machine.last_prediction(),
    );

    reconciler.reconcile(hbt, desired_orders, &mut order_registry)?;

    risk_monitor.check(
        now,
        hbt.depth(0),
        position,
        hbt.orders(0),
        regime_machine.last_prediction(),
    )?;

    recorder.record_if_due(hbt, regime_machine.current())?;
}
```

执行顺序不能调整：

```text
更新状态
→ 撤销禁止订单
→ 将待撤订单继续计入风险
→ 重算预算
→ 生成减仓订单
→ 生成开仓订单
→ 对账
```

---

## 10. 项目目录改造

建议新增：

```text
hftbacktest/examples/regime_grid/
├── mod.rs
├── config.rs
├── feature_engine.rs
├── feature_schema.rs
├── model.rs
├── regime_machine.rs
├── direction_policy.rs
├── order_intent.rs
├── order_registry.rs
├── risk_budget.rs
├── grid_planner.rs
├── order_reconciler.rs
├── risk_monitor.rs
└── metrics.rs

hftbacktest/examples/
├── regime_feature_dump.rs
├── regime_grid_backtest.rs
└── regime_grid_live.rs

model-training/
├── pyproject.toml
├── regime_model/
│   ├── build_labels.py
│   ├── train_glasso.py
│   ├── fista.py
│   ├── calibration.py
│   ├── walk_forward.py
│   ├── evaluate.py
│   └── export_model.py
└── tests/

configs/
├── regime_grid_backtest.yaml
└── regime_grid_live.yaml

models/
└── regime_glasso_v001.json
```

---

## 11. 对现有文件的具体修改

## 11.1 `gridtrading_backtest.rs`

不要直接破坏原始基线示例。

保留：

```text
gridtrading_backtest.rs
```

新增：

```text
regime_grid_backtest.rs
```

构建资产时增加：

```rust
.last_trades_capacity(4096)
```

输出额外字段：

```text
timestamp
mid
p_up
p_sideways
p_down
regime
regime_version
position
target_position
open_long_budget
open_short_budget
reduce_long_budget
reduce_short_budget
forbidden_order_count
risk_off_reason
```

---

## 11.2 `gridtrading_live.rs`

同样新增独立文件：

```text
regime_grid_live.rs
```

将：

```rust
Instrument::new(..., 0)
```

改为：

```rust
Instrument::new(..., 4096)
```

注册 `order_recv_hook`，只设置共享脏标志：

```rust
let order_dirty = Arc::new(AtomicBool::new(false));
```

收到订单变化后：

```rust
order_dirty.store(true, Ordering::Release);
```

主循环看到脏标志后立即执行订单和仓位重算。

---

## 11.3 `types.rs`

实盘最终版本建议扩展：

```rust
pub struct OrderRequest {
    pub order_id: u64,
    pub price: f64,
    pub qty: f64,
    pub side: Side,
    pub time_in_force: TimeInForce,
    pub order_type: OrdType,
    pub reduce_only: bool,
    pub position_side: PositionSide,
}
```

如果不希望立即修改公共API，可以先新增：

```rust
pub struct ExtendedOrderRequest
```

然后只让Binance Futures连接器使用。

---

## 11.4 Binance Futures连接器

下单REST参数增加：

```text
reduceOnly=true|false
positionSide=BOTH|LONG|SHORT
```

MVP建议只支持单向持仓模式：

```text
positionSide=BOTH
```

启动时读取账户持仓模式并验证。若账户是双向持仓模式而策略配置为单向模式，禁止启动。

### 双重保护

```text
策略层：
  减仓数量不得超过当前仓位

交易所层：
  减仓订单设置reduceOnly=true
```

不能只依赖其中一层。

---

## 12. 统一特征实现，避免训练/实盘偏差

不要分别用Python和Rust重复实现特征公式。

正确流程：

```text
hftbacktest NPZ行情
        ↓
Rust regime_feature_dump
        ↓
使用与实盘相同的FeatureEngine
        ↓
输出Parquet/CSV特征
        ↓
Python只负责标签、训练和评估
```

命令设计：

```bash
cargo run --release --example regime_feature_dump -- \
  --config configs/regime_grid_backtest.yaml \
  --output data/features/1000SHIBUSDT
```

训练：

```bash
python -m regime_model.train_glasso \
  --features data/features/1000SHIBUSDT \
  --config configs/regime_model.yaml \
  --output models/regime_glasso_v001.json
```

回测：

```bash
cargo run --release --example regime_grid_backtest -- \
  --config configs/regime_grid_backtest.yaml \
  --model models/regime_glasso_v001.json
```

---

## 13. 完整配置

```yaml
symbol: 1000SHIBUSDT
asset_no: 0

runtime:
  quote_interval_ms: 100
  model_interval_ms: 1000
  record_interval_ms: 1000
  warmup_ms: 300000
  prediction_stale_ms: 3000
  market_data_stale_ms: 2000

model:
  path: models/regime_glasso_v001.json
  expected_type: multinomial_group_lasso
  prediction_horizon_ms: 60000
  require_schema_hash: true

label:
  volatility_multiplier: 0.80
  grid_multiplier: 2.00
  cost_floor_bps: 2.00

regime:
  trend_enter_probability: 0.62
  sideways_enter_probability: 0.58
  enter_margin: 0.15
  sideways_margin: 0.10
  exit_probability: 0.50
  confirmation_count: 3
  minimum_hold_ms: 5000
  opposite_transition_via_uncertain: true

grid:
  relative_half_spread: 0.0005
  relative_grid_interval: 0.0005
  min_grid_step: 0.000001
  open_levels_sideways: 10
  open_levels_trend: 6
  reduce_levels: 4
  order_qty: 1.0
  alpha_multiplier: 0.50
  alpha_max_grid_intervals: 1.0
  reduce_spread_factor: 0.70

position:
  max_long: 10.0
  max_short: 10.0
  edge_full: 0.50
  min_directional_limit_ratio: 0.50
  prevent_direct_reversal: true

risk:
  max_spread_bps: 20.0
  max_position_hard: 10.0
  max_daily_loss: 100.0
  max_drawdown: 150.0
  volatility_shock_multiple: 4.0
  cancel_open_orders_on_uncertain: true
  risk_off_exit_mode: aggressive_reduce_only

orders:
  tif_open: GTX
  tif_reduce_passive: GTX
  tif_risk_off: IOC
  use_exchange_reduce_only: true
  wait_cancel_confirmation: false
  pending_cancel_counts_as_exposure: true

logging:
  output: output/regime_grid
  log_probabilities: true
  log_features: false
  log_order_intent: true
  log_state_transitions: true
```

---

## 14. 回测实验设计

必须使用同一份数据、延迟模型、手续费模型和队列模型比较。

### A：原始对称网格

```text
现有gridtrading
```

### B：规则过滤

```text
EMA/收益阈值判断方向
```

用于判断复杂模型是否真的优于简单规则。

### C：模型只移动alpha

```text
三分类模型
不限制开仓方向
```

用于证明单纯预测中心偏移是否不足。

### D：模型加方向许可

```text
三分类
状态机
禁止逆势开仓
```

### E：完整方案

```text
三分类
状态机
开仓/减仓意图
禁止直接反转
RiskOff
交易所reduce-only语义
```

---

## 15. 回测指标

## 15.1 模型指标

```text
Macro F1
每类Precision/Recall
Kappa
Log Loss
Brier Score
概率校准曲线
UP→DOWN误判率
DOWN→UP误判率
状态平均持续时间
状态切换次数
趋势确认延迟
```

## 15.2 策略指标

```text
净收益
Sharpe
Sortino
最大回撤
最大库存
平均绝对库存
库存持有时间
手续费
成交次数
撤单次数
订单成交率
1s/5s/30s markout
单边行情逆势新增仓位
RiskOff次数
方向策略违规次数
```

最重要的硬指标：

```text
UP状态 OpenShort成交量 == 0
DOWN状态 OpenLong成交量 == 0
Uncertain状态新增开仓量 == 0
减仓订单总量不超过可减仓位
直接从Long翻转Short次数 == 0
直接从Short翻转Long次数 == 0
```

---

## 16. 上线门槛

模型门槛：

```text
UP↔DOWN直接误判率显著低于简单规则
概率校准没有系统性过度自信
不同时间段结果方向一致
特征组和系数没有频繁完全重构
```

策略门槛：

```text
方向策略违规次数必须为0
样本外最大回撤相对基线降低至少20%
单边行情逆势新增仓位相对基线降低至少70%
样本外净收益不得因过滤下降超过10%
样本外Sharpe不得低于基线
多个日期和波动区间均通过
```

上述百分比是首版Go/No-Go标准，不代表收益承诺。若不满足，禁止进入实盘。

---

## 17. 测试清单

## 17.1 单元测试

```text
特征窗口边界
特征标准化
模型softmax数值稳定
schema hash校验
状态连续确认
状态滞回
UP不能直接跳DOWN
订单ID编码/解码
订单意图识别
四类预算计算
部分成交后的预算重算
撤单中的订单继续计入风险
```

## 17.2 属性测试

随机生成持仓和订单，始终满足：

```text
pending_reduce_long <= long_position
pending_reduce_short <= short_position
position > 0时不能生成OpenShort
position < 0时不能生成OpenLong
RiskOff不能生成Open订单
```

## 17.3 集成测试场景

1. 横盘往返。
2. 突然上涨。
3. 突然下跌。
4. 上涨切换下跌。
5. 深度断流。
6. 模型文件损坏。
7. 订单部分成交。
8. 撤单延迟。
9. 成交后仓位刚好归零。
10. 归零前行情已反向。
11. 程序重启后存在遗留订单。
12. 账户持仓和本地持仓不一致。

---

## 18. 运行阶段

### 阶段1：数据和基线

交付：

```text
FeatureEngine
feature dump
原始网格基线报告
数据质量报告
```

### 阶段2：论文模型

交付：

```text
G-LASSO训练器
Walk-forward验证
模型JSON
概率校准
模型评估报告
```

### 阶段3：方向感知回测

交付：

```text
RegimeMachine
DirectionPolicy
OrderIntent
RiskBudget
GridPlanner
OrderReconciler
完整A-E对照回测
```

### 阶段4：影子运行

真实行情运行，但不下单：

```text
记录三类概率
记录状态切换
记录理论订单
记录理论持仓
对比未来真实走势
```

### 阶段5：模拟盘

要求：

```text
方向违规为0
仓位和订单持续对账
断流后正确RiskOff
重启后正确接管遗留订单
```

### 阶段6：小额实盘

限制：

```text
单标的
低最大仓位
低订单层数
禁止自动扩大参数
模型固定版本
人工可立即停机
```

---

## 19. 故障处理

| 故障 | 动作 |
|---|---|
| 模型无法加载 | 禁止启动交易 |
| 模型过期 | RiskOff |
| 特征未预热 | 不开仓 |
| 深度不完整 | 不更新网格 |
| 成交数据缺失 | 降级或Uncertain |
| 仓位对账失败 | 取消开仓单并RiskOff |
| 撤单超时 | 订单继续计入风险 |
| 本地订单未知 | 从交易所重建订单注册表 |
| 发现非本策略订单 | 不自动接管，告警并RiskOff |
| 日亏损超限 | 取消开仓并清仓 |
| 模型概率异常 | Uncertain或RiskOff |

---

## 20. 最终验收定义

系统只有同时满足以下条件才算完成：

### 模型层

- 能稳定输出三类概率；
- 能在样本外数据上通过时间序列验证；
- 模型文件可版本化和回滚；
- Rust和训练端对同一样本的概率误差小于 `1e-10`。

### 决策层

- 状态具有确认、滞回和过期机制；
- 相反趋势必须经过 `Uncertain`；
- 不确定时不新增仓位。

### 订单层

- 每个订单都有明确意图；
- 持仓非零时不允许直接反向开仓；
- 减仓数量永远不超过当前可减仓位；
- 状态切换时先撤禁止订单，再生成新订单；
- 待撤订单继续占用风险预算。

### 风控层

- 数据、模型、账户、订单任一不可信时进入 `RiskOff`；
- 交易所reduce-only和策略预算同时生效；
- 重启后可恢复仓位和订单状态。

### 策略层

- 单边行情逆势新增仓位显著下降；
- 样本外最大回撤优于原始网格；
- 收益没有被过度过滤；
- 所有方向策略违规指标为0。

---

## 21. 推荐的最终MVP

```text
论文三分类结构
+ 24个五组高频特征
+ G-LASSO多项Logit
+ 1秒推理
+ 100毫秒报价
+ 五状态RegimeMachine
+ 开仓方向许可
+ Open/Reduce订单意图
+ 持仓非零禁止直接反转
+ 逻辑reduce-only
+ Binance reduceOnly
+ RiskOff
+ Purged Walk-Forward回测
```

真正解决问题的不是“预测上涨还是下跌”本身，而是将预测结果转化为不可绕过的订单权限：

```text
趋势判断错误时，风险可控；
趋势判断正确时，逆势网格确实停止；
任何情况下，减仓能力始终保留；
订单不得在平仓后继续穿透成反向开仓。
```
