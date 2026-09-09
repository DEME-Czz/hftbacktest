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

`GridStrategy` is a double-sided grid market-making strategy. It keeps the strategy layer exchange-independent while the live runtime supplies normalized market depth, open orders and the current net position.

The current P0 market-making loop is:

```text
Mid Price
   |
   v
Inventory Ratio = clamp(position / max_position, -1, 1)
   |
   v
Reservation Price
   |
   v
Bid / Ask Grid
   |
   v
Requote hysteresis + minimum quote lifetime
   |
   v
StrategyCommand
   |
   v
RiskGate / LiveExecutor
   |
   v
Exchange fill / position update
   |
   +-----------------------------> Inventory Ratio
```

The implementation deliberately uses `max_position` as the inventory normalization denominator. `order_qty` is only the base order size. Changing `order_qty` therefore does not accidentally multiply reservation-price skew.

Inventory has three operating zones:

- normal: `abs(position / max_position) < inventory_reduce_threshold`; both sides use the configured base quantity;
- defensive: between `inventory_reduce_threshold` and `inventory_stop_threshold`; the risk-increasing side is reduced to half size while the inventory-reducing side remains at base size;
- reduce: at or above `inventory_stop_threshold`; the risk-increasing side is removed and only inventory-reducing quotes remain.

In the reduce zone, the total quantity of the inventory-reducing ladder is capped by the current absolute position. Therefore a complete fill of every reducing quote cannot cross through flat and accidentally create a new position in the opposite direction. Risk-increasing stale quotes are canceled immediately in this zone rather than waiting for the normal minimum quote lifetime.

`max_position` remains a hard strategy boundary, and the live `RiskGate` remains the final hard exposure guard including same-side pending order exposure.

### Requote controls

Grid quotes are not canceled for every small market movement.

- `requote_ticks` keeps an existing quote when its price remains close enough to the new target and its target quantity has not changed.
- `min_quote_lifetime_ms` prevents normal strategy repricing from canceling a fresh order before it has had a minimum opportunity to rest in the book.
- when a quote really must move, the strategy sends the cancel first and waits for the exchange terminal update before submitting the replacement. This avoids cancel+submit bursts and helps preserve a bounded open-order count.

Safety cancellation and shutdown cancellation are outside the strategy layer and are not delayed by these quote-lifetime controls.

### Account-driven quote refresh

The live runtime tracks quote dirtiness separately from market-depth dirtiness. Depth changes, meaningful order updates and position changes can mark quotes dirty. A fill temporarily makes account state unready until a sufficiently current position update arrives; once the position is current and the safety gate allows trading, the service immediately reevaluates inventory-aware quotes instead of waiting only for a later depth batch.

This closes the minimum live loop:

```text
Quote -> Fill -> Position -> Inventory -> Requote
```

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

Future fair-value and adverse-selection components should remain independent of transport concerns. Candidates include microprice, volatility-aware spread control and order-flow toxicity, but those are intentionally outside the current P0 stabilization scope.
