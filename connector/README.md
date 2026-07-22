# HftBacktest - Connector

Connector provides a single point of communication with exchanges, brokers, or data feed providers.
It is designed to manage multiple bots, allowing each bot to connect to several different connectors simultaneously.

![architecture](https://github.com/nkaz001/hftbacktest/blob/master/docs/images/arch.png)

## Supported Exchanges

**CAUTION: Use at your own risk. Live trading features may not function correctly in all cases.
Please report any issues you encounter by submitting them to the Issues.**

Supported connectors include:

- `binancefutures`: Binance USD-M Futures. Symbols must be lowercase.
- `binancespot`: Binance Spot. Symbols must be lowercase.
- `bybit`: Bybit. Symbols must be uppercase.

## Getting Started

1. Clone the repository:

    ```
    git clone https://github.com/nkaz001/hftbacktest.git
    ```

2. Build Connector. The initial release build can take several minutes because Cargo compiles the dependency graph with optimizations. Subsequent builds normally reuse the cache. The executable is generated at `target/release/connector`:

    ```
    cargo build --release --package connector
    ```

3. Configure an exchange settings file. See the [`connector/examples`](examples) directory. Do not commit API credentials. Use an API key restricted to the required trading permissions, with withdrawals disabled and an IP allowlist where possible.

4. Run Connector. Every runtime parameter is stored in one TOML file; the CLI accepts only its path:

    ```
    connector <CONFIG>
    ```

   The TOML fields `name` and `connector` select the local IPC instance and connector implementation.

   Example:

    ```bash
    ./target/release/connector \
        connector/examples/binancefutures-demo.toml
    ```

   Other exchange URLs, credentials, and order prefixes remain in that same file.

Note: Since Connector communicates with bots via shared memory, both Connector and the bots must run on the same machine.

## Binance Futures WebSocket configuration

The Binance Futures connector supports separate public and private WebSocket URLs:

```toml
market_stream_url = "wss://fstream.binance.com/ws"
user_stream_url = "wss://fstream.binance.com/private/ws"
```

- `market_stream_url` is the complete WebSocket endpoint used for public subscription messages.
- `user_stream_url` is the prefix to which Connector appends `/<listenKey>`.
- The legacy `stream_url` field remains supported for environments where public and private
  streams share one endpoint, including the current Demo configuration.
- Do not configure `user_stream_url` as `wss://fstream.binance.com/private`; Connector expects the
  `/ws` component to already be present.
- The public Streams subscription protocol used by this connector has been verified against
  `wss://fstream.binance.com/ws`. Although `/market/ws` accepts a subscription request, it did not
  publish the DOGEUSDT depth stream during integration verification and is therefore not used by
  the production example.

## Binance Futures Demo environment

Use Demo to validate credentials, connectivity, exchange filters, and order behavior before
enabling production trading. Binance symbols are lowercase inside the bot (`dogeusdt`).

1. Copy the tracked template, then put Binance Demo credentials in the ignored local configuration file:

    ```bash
    cp connector/examples/binancefutures-demo.toml.example \
        connector/examples/binancefutures-demo.toml
    ```

    Edit the copied file:

    ```toml
    order_prefix = "test"
    api_key = "<BINANCE_DEMO_API_KEY>"
    secret = "<BINANCE_DEMO_SECRET>"
    ```

2. Build Connector:

    ```bash
    cargo build --release --package connector
    ```

3. Start the connector first. Binance Futures defaults to the HTTP CONNECT proxy
   `http://127.0.0.1:7890`; REST and both WebSocket streams share `proxy_url` from the TOML file.
   Set `proxy_url = ""` only when direct access is required:

    ```bash
    ulimit -n 4096
    RUST_LOG=info \
    ./target/release/connector \
        connector/examples/binancefutures-demo.toml
    ```

The checked-in `gridtrading_live` example is intentionally configured for the production IPC name;
do not use it with this Demo connector without first changing its connector name.

## Binance Futures production environment

The live grid example trades `DOGEUSDT` through the IPC name `binancefutures-prod`. The `NAME` in
the connector command and the name in `gridtrading_live.rs` must match exactly.

The connector configuration does not define the instruments to trade. A top-level
`symbols = [...]` entry in a connector TOML file is currently ignored: `RuntimeConfig` reads only
`name` and `connector`, while the Binance Futures connector reads its endpoint, proxy, credential,
and order-prefix fields. Instruments are registered at runtime by each bot or TUI through IPC. For
example, `gridtrading_live` reads `symbol` from
`hftbacktest/examples/gridtrading-live.toml`. Consequently, it is normal for the connector's first
account-snapshot log to show `symbols={}` when its private stream connects before a bot registers.
After `RegisterInstrument` is received, the connector logs another account snapshot containing the
newly registered symbol.

1. Copy the production template and enter a production API key and secret:

    ```bash
    cp connector/examples/binancefutures-prod.toml.example \
        connector/examples/binancefutures-prod.toml
    ```

   Use a dedicated key with USD-M Futures trading enabled, withdrawals disabled, and an IP
   allowlist. Never reuse Demo credentials in the production file.

2. Build both binaries:

    ```bash
    cargo build --release --package connector
    cargo build --release --package hftbacktest --example gridtrading_live
    ```

3. Start Connector first:

    ```bash
    ulimit -n 4096
    RUST_LOG=info \
    RUST_BACKTRACE=1 \
    ./target/release/connector \
        connector/examples/binancefutures-prod.toml
    ```

4. Wait until Connector is running without connection errors. In another terminal, start the
   strategy:

    ```bash
    RUST_LOG=info \
    RUST_BACKTRACE=1 \
    ./target/release/examples/gridtrading_live \
        hftbacktest/examples/gridtrading-live.toml
    ```

The live strategy parameters are documented field-by-field in `hftbacktest/examples/gridtrading-live.toml`. Before changing the symbol, update its tick size, lot size, minimum grid step, order quantity, position limit, dataset, and model paths. Every non-reduce-only order must also satisfy Binance's minimum notional.
Before placing orders, the example waits up to 30 seconds for both sides of the market depth and a
received, finite quote-asset wallet balance. Zero position and zero balance are valid synchronized
states. On timeout, the error includes `account_ready`, `best_bid_tick`, and `best_ask_tick`, so an
account-stream failure can be distinguished from a market-stream failure.

### Current account-state behavior

- When the private stream connects, the Binance Futures connector fetches quote-asset wallet
  balances and active positions from `/fapi/v3/account`. This first request can run with an empty
  symbol set because instruments are registered by bots rather than the connector TOML.
- Whenever a bot registers a symbol, the connector fetches the account snapshot again for that
  symbol. Registered symbols omitted by Binance are initialized with zero position.
- Subsequent wallet-balance and position changes are taken from `ACCOUNT_UPDATE` messages.
- `StateValues.balance` is the wallet balance of the symbol's quote asset (for example, USDT for
  DOGEUSDT). Fees, trade count, volume, and value are accumulated from this bot's unique Binance
  fills since the bot started. The example still limits inventory using `max_position`; it does not
  size orders from available margin.

### Trading safety and shutdown behavior

- When a symbol is registered at startup, after reconnection, or later at runtime, the Binance Futures connector cancels all open orders for that symbol on the exchange. This is not limited to orders with `order_prefix`.
- Restarting Connector repeats this cancel-all behavior. Inspect the exchange account before every
  restart and do not restart while orders that must remain active are present.
- `LiveBot::close()` currently performs no exchange cleanup. Stopping the strategy does not guarantee that open orders are canceled or that positions are flattened.
- Verify the account mode, margin mode, leverage, price/quantity filters, minimum notional, and available margin before enabling real trading.
- Use a new IPC `NAME` if a previous abnormal termination left unusable shared-memory state.

### Troubleshooting HTTP 418

An HTTP `418` response from `/fapi/v3/account` is independent of whether the log shows an empty
symbol set: that endpoint is account-wide and the connector does not send `symbols` in the REST
request. Binance uses HTTP 418 for an automatically banned source IP after repeated rate-limit
violations. Stop all connector/strategy instances using the same direct or proxy egress IP, do not
retry until Binance's `Retry-After` period has elapsed, and check whether other programs share the
same proxy. Restarting the connector during the ban only creates more failed requests. The official
rate-limit guidance is documented at
<https://developers.binance.com/en/docs/products/spot/rest-api#http-return-codes>.

## Connector Implementation Guide

If a connector adheres to the IPC protocol, it does not have to be implemented in the same manner as Connector.
However, following this implementation makes it easier to develop additional connectors.

To implement a connector, you mainly need to implement two traits: `Connector` and `ConnectorBuilder`.

For further details, please see the documentation.
