# Market data collector

Collector only connects to public market-data WebSockets. It does not load account credentials,
register an instrument with Connector IPC, or expose order submission methods.

All parameters live in one documented TOML file:

```bash
cargo build --release --package collector
./target/release/collector collector/examples/binancefutures.toml
```

如果本机不能直连币安，可在配置中设置 HTTP CONNECT 代理：

```toml
proxy = "127.0.0.1:7890"
```

直连可用时删除或注释该参数。

Edit `symbols` to collect multiple markets in one process. Output is written below `output_path`
and rotated by symbol and UTC date. Stop it with Ctrl-C. This raw exchange feed is not the same
schema as the Alpha training CSV; it must be reconstructed into synchronized order-book snapshots
before it is passed to `train_alpha`.

No Connector or live strategy process is required for this collector.
