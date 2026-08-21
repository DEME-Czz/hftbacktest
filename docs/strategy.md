# Strategy Layer

The strategy layer is exchange-independent. A strategy receives normalized market/account state and returns execution commands. It must not depend on Binance, REST, WebSocket, Tokio, or backtest transport details.

## Layers

```text
Market data / account state
          |
          v
    MarketContext
          |
          v
       Strategy
          |
          v
   StrategyCommand
          |
     +----+----+
     |         |
 Backtest     Live
 Simulator   Executor
```

## Built-in strategies

### GridStrategy

`GridStrategy` is migrated from `master`'s `hftbacktest/examples/algo.rs` grid market-making strategy. The migrated version preserves the core behavior:

- relative half spread
- relative grid interval
- multiple bid/ask grid levels
- inventory-based skew
- maximum position limit
- cancel stale quotes
- submit missing GTX limit quotes

The old `gridtrading_live.rs` runtime was not migrated because it depended on the removed Iceoryx/LiveBot infrastructure.

## Extension rule

A new strategy should:

1. live under `engine/src/strategy/<name>.rs`;
2. implement `Strategy<MD>`;
3. consume only `MarketContext`;
4. emit only `StrategyCommand`;
5. add a `BuiltinStrategyConfig` / `BuiltinStrategy` variant only if it is maintained as a built-in strategy;
6. contain no exchange-specific code.

Do not put Binance DTOs, API keys, REST calls, WebSocket handling, or runtime channels inside a strategy.

## Current built-in registry

```text
BuiltinStrategy
└── Grid
```

Future candidates can be added independently, for example:

```text
BuiltinStrategy
├── Grid
├── Imbalance
├── Microprice
└── OrderFlow
```

The runtime/executor should not need strategy-specific branches beyond constructing the selected built-in strategy from configuration.
