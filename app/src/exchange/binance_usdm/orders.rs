use std::sync::{Arc, Mutex};

use hashbrown::HashMap;
use hftbacktest::types::{Order, OrderId, Status};
use tracing::error;

use crate::exchange::binance_usdm::{
    BinanceFuturesError,
    id::{ClientOrderIdCodec, ClientOrderIdError, RefSymbolOrderId, SymbolOrderId},
    now_ns,
    protocol::{rest::OrderResponse, stream::OrderTradeUpdate},
};

#[derive(Clone, Debug)]
struct OrderExt {
    symbol: String,
    order: Order,
    removed_by_ws: bool,
    removed_by_rest: bool,
}

pub(crate) type SharedOrderManager = Arc<Mutex<OrderManager>>;

pub type ClientOrderId = String;

/// Binance has separated channels for REST APIs and Websocket. Order responses are delivered
/// through these channels, with no guaranteed order of transmission. To prevent duplicate handling
/// of order responses, such as order deletion due to cancellation or fill, OrderManager manages the
/// order states before transmitting the responses to a live bot.
///
/// Deletions must be confirmed by both channels. If not, differences in response times could result
/// in attempts to update an order that has already been deleted, potentially creating a ghost order
/// unintentionally.
///
/// To handle this, the `client_order_id` should include a random ID to differentiate it, even when
/// the order ID is the same(bot's order id). This is necessary because the order deletion is
/// immediately notified to the bot, but the Connector must still retain the `client_order_id` in
/// case an update arrives later from the other channel, which has not yet sent the deletion
/// message.
#[derive(Clone, Debug)]
pub struct OrderManager {
    client_order_ids: ClientOrderIdCodec,
    orders: HashMap<ClientOrderId, OrderExt>,
    order_id_map: HashMap<SymbolOrderId, ClientOrderId>,
}

impl OrderManager {
    pub(crate) fn new(client_order_ids: ClientOrderIdCodec) -> Self {
        Self {
            client_order_ids,
            orders: Default::default(),
            order_id_map: Default::default(),
        }
    }

    pub fn update_from_ws(
        &mut self,
        resp: &OrderTradeUpdate,
    ) -> Result<Option<Order>, BinanceFuturesError> {
        match self.client_order_ids.decode(&resp.order.client_order_id) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(BinanceFuturesError::PrefixUnmatched),
            Err(ClientOrderIdError::Malformed | ClientOrderIdError::InvalidPrefix) => {
                return Err(BinanceFuturesError::MalformedClientOrderId);
            }
        }
        let order_ext = self
            .orders
            .get_mut(&resp.order.client_order_id)
            .ok_or(BinanceFuturesError::OrderNotFound)?;

        let already_removed = order_ext.removed_by_ws || order_ext.removed_by_rest;
        if !already_removed && resp.transaction_time * 1_000_000 >= order_ext.order.exch_timestamp {
            order_ext.order.qty = resp.order.original_qty;
            order_ext.order.leaves_qty =
                resp.order.original_qty - resp.order.order_filled_accumulated_qty;
            order_ext.order.side = resp.order.side;
            order_ext.order.time_in_force = resp.order.time_in_force;
            order_ext.order.exch_timestamp = resp.transaction_time * 1_000_000;
            order_ext.order.status = resp.order.order_status;
            if !matches!(
                order_ext.order.status,
                Status::New | Status::PartiallyFilled
            ) {
                // A terminal exchange update is authoritative. Any local submit/cancel request
                // is finished and must not keep this order visible as pending.
                order_ext.order.req = Status::None;
            }
            order_ext.order.exec_price_tick =
                (resp.order.last_filled_price / order_ext.order.tick_size).round() as i64;
            order_ext.order.exec_qty = resp.order.order_last_filled_qty;
            order_ext.order.order_type = resp.order.order_type;
        }

        let result = if already_removed {
            None
        } else {
            Some(order_ext.order.clone())
        };

        if order_ext.order.status != Status::New
            && order_ext.order.status != Status::PartiallyFilled
        {
            order_ext.removed_by_ws = true;
            if !already_removed {
                self.order_id_map.remove(&RefSymbolOrderId::new(
                    &order_ext.symbol,
                    order_ext.order.order_id,
                ));
            }

            if order_ext.removed_by_ws && order_ext.removed_by_rest {
                self.orders.remove(&resp.order.client_order_id);
            }
        }

        Ok(result)
    }

    pub fn update_submit_fail(
        &mut self,
        client_order_id: &ClientOrderId,
        error: &BinanceFuturesError,
    ) -> Option<Order> {
        match error {
            BinanceFuturesError::OrderError { code: -5022, .. } => {
                // GTX rejection.
            }
            BinanceFuturesError::OrderError { code: -1008, .. } => {
                error!(
                    "Server is currently overloaded with other requests. Please try again in a few minutes."
                );
            }
            BinanceFuturesError::OrderError { code: -2019, .. } => {
                error!("Margin is insufficient.");
            }
            BinanceFuturesError::OrderError { code: -1015, .. } => {
                error!("Too many new orders; current limit is 300 orders per TEN_SECONDS.");
            }
            error => {
                error!(?error, "submit error");
            }
        }
        self.update_from_rest_fail(client_order_id, Some(Status::Expired))
    }

    pub fn update_cancel_fail(
        &mut self,
        client_order_id: &ClientOrderId,
        error: &BinanceFuturesError,
    ) -> Option<Order> {
        error!(?error, "cancel error");
        self.update_from_rest_fail(client_order_id, None)
    }

    pub fn update_from_rest_fail(
        &mut self,
        client_order_id: &ClientOrderId,
        status: Option<Status>,
    ) -> Option<Order> {
        let order_ext = self.orders.get_mut(client_order_id)?;

        let already_removed = order_ext.removed_by_ws || order_ext.removed_by_rest;
        if let Some(status) = status {
            order_ext.order.status = status;
        }
        order_ext.order.req = Status::None;

        let result = if already_removed {
            None
        } else {
            Some(order_ext.order.clone())
        };

        if order_ext.order.status != Status::New
            && order_ext.order.status != Status::PartiallyFilled
        {
            order_ext.removed_by_rest = true;
            if !already_removed {
                self.order_id_map.remove(&RefSymbolOrderId::new(
                    &order_ext.symbol,
                    order_ext.order.order_id,
                ));
            }

            if order_ext.removed_by_ws && order_ext.removed_by_rest {
                self.orders.remove(client_order_id);
            }
        }

        result
    }

    pub fn update_from_rest(
        &mut self,
        client_order_id: &ClientOrderId,
        resp: &OrderResponse,
    ) -> Option<Order> {
        self.update_from_rest_with_fill(client_order_id, resp).0
    }

    pub fn update_from_rest_with_fill(
        &mut self,
        client_order_id: &ClientOrderId,
        resp: &OrderResponse,
    ) -> (Option<Order>, bool) {
        let Some(order_ext) = self.orders.get_mut(client_order_id) else {
            return (None, false);
        };

        let already_removed = order_ext.removed_by_ws || order_ext.removed_by_rest;
        let response_is_current =
            !already_removed && resp.update_time * 1_000_000 >= order_ext.order.exch_timestamp;
        let known_filled_qty = (order_ext.order.qty - order_ext.order.leaves_qty).max(0.0);
        let fill_tolerance =
            resp.executed_qty.abs().max(known_filled_qty.abs()).max(1.0) * f64::EPSILON * 8.0;
        let has_new_fill = response_is_current
            && resp.executed_qty.is_finite()
            && resp.executed_qty > known_filled_qty + fill_tolerance;
        if response_is_current {
            order_ext.order.qty = resp.orig_qty;
            order_ext.order.leaves_qty = resp.orig_qty - resp.cum_qty;
            order_ext.order.side = resp.side;
            order_ext.order.time_in_force = resp.time_in_force;
            order_ext.order.exch_timestamp = resp.update_time * 1_000_000;
            order_ext.order.status = resp.status;
            order_ext.order.exec_qty = resp.executed_qty;
            order_ext.order.order_type = resp.ty;
            order_ext.order.req = Status::None;
        }

        let result = if already_removed {
            None
        } else {
            Some(order_ext.order.clone())
        };

        if order_ext.order.status != Status::New
            && order_ext.order.status != Status::PartiallyFilled
        {
            order_ext.removed_by_rest = true;
            if !already_removed {
                self.order_id_map.remove(&RefSymbolOrderId::new(
                    &order_ext.symbol,
                    order_ext.order.order_id,
                ));
            }

            if order_ext.removed_by_ws && order_ext.removed_by_rest {
                self.orders.remove(client_order_id);
            }
        }

        (result, has_new_fill)
    }

    pub fn prepare_client_order_id(&mut self, symbol: String, order: Order) -> Option<String> {
        let symbol_order_id = SymbolOrderId::new(symbol.clone(), order.order_id);
        if self.order_id_map.contains_key(&symbol_order_id) {
            return None;
        }

        let client_order_id = self.client_order_ids.encode(order.order_id);
        if self.orders.contains_key(&client_order_id) {
            return None;
        }

        self.order_id_map
            .insert(symbol_order_id, client_order_id.clone());
        self.orders.insert(
            client_order_id.clone(),
            OrderExt {
                symbol,
                order,
                removed_by_ws: false,
                removed_by_rest: false,
            },
        );
        Some(client_order_id)
    }

    pub fn get_client_order_id(&self, symbol: &str, order_id: OrderId) -> Option<String> {
        self.order_id_map
            .get(&RefSymbolOrderId::new(symbol, order_id))
            .cloned()
    }

    pub(crate) fn reconcile_open_orders(
        &mut self,
        symbol: &str,
        tick_size: f64,
        responses: &[OrderResponse],
    ) -> Result<Vec<Order>, BinanceFuturesError> {
        if !tick_size.is_finite() || tick_size <= 0.0 {
            return Err(BinanceFuturesError::InvalidRequest);
        }

        let mut recovered = Vec::new();
        let mut recovered_ids = hashbrown::HashSet::new();
        let mut recovered_client_ids = hashbrown::HashSet::new();
        for response in responses {
            let order_id = match self.client_order_ids.decode(&response.client_order_id) {
                Ok(Some(order_id)) => order_id,
                Ok(None) => continue,
                Err(ClientOrderIdError::Malformed | ClientOrderIdError::InvalidPrefix) => {
                    return Err(BinanceFuturesError::MalformedClientOrderId);
                }
            };
            if response.symbol != symbol
                || !response.price.is_finite()
                || !response.orig_qty.is_finite()
                || !response.cum_qty.is_finite()
                || response.orig_qty <= 0.0
                || response.cum_qty < 0.0
                || response.cum_qty > response.orig_qty
                || !matches!(
                    response.side,
                    hftbacktest::types::Side::Buy | hftbacktest::types::Side::Sell
                )
                || !matches!(response.status, Status::New | Status::PartiallyFilled)
            {
                return Err(BinanceFuturesError::InvalidRequest);
            }
            if !recovered_ids.insert(order_id)
                || !recovered_client_ids.insert(response.client_order_id.clone())
            {
                return Err(BinanceFuturesError::OrderRecoveryConflict);
            }
            if let Some(existing_client_order_id) = self
                .order_id_map
                .get(&RefSymbolOrderId::new(symbol, order_id))
                && existing_client_order_id != &response.client_order_id
            {
                return Err(BinanceFuturesError::OrderRecoveryConflict);
            }

            let mut order = Order::new(
                order_id,
                (response.price / tick_size).round() as i64,
                tick_size,
                response.orig_qty,
                response.side,
                response.ty,
                response.time_in_force,
            );
            order.leaves_qty = response.orig_qty - response.cum_qty;
            order.exec_qty = response.executed_qty;
            order.exch_timestamp = response.update_time.saturating_mul(1_000_000);
            order.status = response.status;
            recovered.push((response.client_order_id.clone(), order));
        }

        let snapshot_client_ids: hashbrown::HashSet<_> = recovered
            .iter()
            .map(|(client_order_id, _)| client_order_id.clone())
            .collect();
        let stale_client_ids: Vec<_> = self
            .orders
            .iter()
            .filter(|(client_order_id, order)| {
                order.symbol == symbol
                    && order.order.active()
                    && !snapshot_client_ids.contains(*client_order_id)
            })
            .map(|(client_order_id, _)| client_order_id.clone())
            .collect();

        let mut updates = Vec::with_capacity(recovered.len() + stale_client_ids.len());
        for client_order_id in stale_client_ids {
            if let Some(mut stale) = self.orders.remove(&client_order_id) {
                self.order_id_map
                    .remove(&RefSymbolOrderId::new(&stale.symbol, stale.order.order_id));
                stale.order.status = Status::None;
                stale.order.req = Status::None;
                updates.push(stale.order);
            }
        }

        for (client_order_id, order) in recovered {
            let symbol_order_id = SymbolOrderId::new(symbol.to_string(), order.order_id);
            self.order_id_map
                .insert(symbol_order_id, client_order_id.clone());
            self.orders.insert(
                client_order_id,
                OrderExt {
                    symbol: symbol.to_string(),
                    order: order.clone(),
                    removed_by_ws: false,
                    removed_by_rest: false,
                },
            );
            updates.push(order);
        }
        Ok(updates)
    }

    pub(crate) fn reconcile_all_open_orders(
        &mut self,
        snapshots: &[(String, f64, Vec<OrderResponse>)],
    ) -> Result<Vec<(String, Vec<Order>)>, BinanceFuturesError> {
        let backup = self.clone();
        let mut reconciled = Vec::with_capacity(snapshots.len());
        for (symbol, tick_size, responses) in snapshots {
            match self.reconcile_open_orders(symbol, *tick_size, responses) {
                Ok(orders) => reconciled.push((symbol.clone(), orders)),
                Err(error) => {
                    *self = backup;
                    return Err(error);
                }
            }
        }
        Ok(reconciled)
    }

    pub fn gc(&mut self) {
        let now = now_ns();
        let stale_ts = now - 300_000_000_000;
        let stale_ids: Vec<(_, _)> = self
            .orders
            .iter()
            .filter(|&(_, wrapper)| {
                wrapper.order.status != Status::New
                    && wrapper.order.status != Status::PartiallyFilled
                    && wrapper.order.status != Status::Unsupported
                    && wrapper.order.exch_timestamp < stale_ts
            })
            .map(|(client_order_id, wrapper)| {
                (
                    client_order_id.clone(),
                    SymbolOrderId::new(wrapper.symbol.clone(), wrapper.order.order_id),
                )
            })
            .collect();
        for (client_order_id, symbol_order_id) in stale_ids {
            self.orders.remove(&client_order_id);
            self.order_id_map.remove(&symbol_order_id);
        }
    }

    pub fn active_orders(&self, symbol: &str) -> Vec<Order> {
        self.orders
            .values()
            .filter(|order_ext| {
                let has_client_order_id = self.order_id_map.contains_key(&RefSymbolOrderId::new(
                    &order_ext.symbol,
                    order_ext.order.order_id,
                ));
                order_ext.symbol == symbol
                    && has_client_order_id
                    && (order_ext.order.active() || order_ext.order.pending())
            })
            .map(|order_ext| order_ext.order.clone())
            .collect()
    }
}
