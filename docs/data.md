# Data

Collector 与 Live 共用 `hft_app::exchange::binance_usdm` Adapter，因此同一个 Binance 消息使用同一种标准化 Event 语义。Collector 只实现 `MarketDataSource` 用例，不会因配置中存在凭证而启动账户流。

Engine 只读取本地 `.npy/.npz` 或内存 Data；S3/AWS 已从 Engine 删除。

Collector CSV 字段：

```text
symbol,ev,exch_ts,local_ts,px,qty,order_id,ival,fval
```

输出文件启动时立即写入并 flush 表头，运行期间每秒 flush。`data/` 是本地运行数据目录，已从 Git 排除。
