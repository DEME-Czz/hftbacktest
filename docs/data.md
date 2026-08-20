# Data

Collector 与 Live 共用 `app::binancefutures` Adapter，因此同一个 Binance 消息使用同一种标准化 Event 语义。

Engine 只读取本地 `.npy/.npz` 或内存 Data；S3/AWS 已从 Engine 删除。

Collector 当前 CSV 字段：

```text
symbol,ev,exch_ts,local_ts,px,qty,order_id,ival,fval
```
