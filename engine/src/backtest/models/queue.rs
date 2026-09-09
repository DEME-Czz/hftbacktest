use std::{any::Any, marker::PhantomData};

use crate::{
    depth::MarketDepth,
    types::{AnyClone, Order, Side},
};

/// Estimates the position of a resting order in an L2 price-level queue.
pub trait QueueModel<MD>
where
    MD: MarketDepth,
{
    fn new_order(&self, order: &mut Order, depth: &MD);
    fn trade(&self, order: &mut Order, qty: f64, depth: &MD);
    fn depth(&self, order: &mut Order, prev_qty: f64, new_qty: f64, depth: &MD);
    fn is_filled(&self, order: &mut Order, depth: &MD) -> f64;
}

pub struct RiskAdverseQueueModel<MD>(PhantomData<MD>);

impl AnyClone for f64 {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl<MD> RiskAdverseQueueModel<MD> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<MD> QueueModel<MD> for RiskAdverseQueueModel<MD>
where
    MD: MarketDepth,
{
    fn new_order(&self, order: &mut Order, depth: &MD) {
        let front_q_qty = if order.side == Side::Buy {
            depth.bid_qty_at_tick(order.price_tick)
        } else {
            depth.ask_qty_at_tick(order.price_tick)
        };
        order.q = Box::new(front_q_qty);
    }

    fn trade(&self, order: &mut Order, qty: f64, _depth: &MD) {
        *order.q.as_any_mut().downcast_mut::<f64>().unwrap() -= qty;
    }

    fn depth(&self, order: &mut Order, _prev_qty: f64, new_qty: f64, _depth: &MD) {
        let front = order.q.as_any_mut().downcast_mut::<f64>().unwrap();
        *front = front.min(new_qty);
    }

    fn is_filled(&self, order: &mut Order, depth: &MD) -> f64 {
        let front = order.q.as_any_mut().downcast_mut::<f64>().unwrap();
        let exec = (-*front / depth.lot_size()).round() as i64;
        if exec > 0 {
            *front = 0.0;
            exec as f64 * depth.lot_size()
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct QueuePos {
    front_q_qty: f64,
    cum_trade_qty: f64,
}

impl AnyClone for QueuePos {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub trait Probability {
    fn prob(&self, front: f64, back: f64) -> f64;
}

pub struct ProbQueueModel<P, MD>
where
    P: Probability,
{
    prob: P,
    _md_marker: PhantomData<MD>,
}

impl<P, MD> ProbQueueModel<P, MD>
where
    P: Probability,
{
    pub fn new(prob: P) -> Self {
        Self {
            prob,
            _md_marker: PhantomData,
        }
    }
}

impl<P, MD> QueueModel<MD> for ProbQueueModel<P, MD>
where
    P: Probability,
    MD: MarketDepth,
{
    fn new_order(&self, order: &mut Order, depth: &MD) {
        let q = QueuePos {
            front_q_qty: if order.side == Side::Buy {
                depth.bid_qty_at_tick(order.price_tick)
            } else {
                depth.ask_qty_at_tick(order.price_tick)
            },
            ..Default::default()
        };
        order.q = Box::new(q);
    }

    fn trade(&self, order: &mut Order, qty: f64, _depth: &MD) {
        let q = order.q.as_any_mut().downcast_mut::<QueuePos>().unwrap();
        q.front_q_qty -= qty;
        q.cum_trade_qty += qty;
    }

    fn depth(&self, order: &mut Order, prev_qty: f64, new_qty: f64, _depth: &MD) {
        let q = order.q.as_any_mut().downcast_mut::<QueuePos>().unwrap();
        let mut chg = prev_qty - new_qty - q.cum_trade_qty;
        q.cum_trade_qty = 0.0;
        if chg < 0.0 {
            q.front_q_qty = q.front_q_qty.min(new_qty);
            return;
        }
        if chg.is_nan() {
            chg = 0.0;
        }
        let front = q.front_q_qty;
        let back = prev_qty - front;
        let mut prob = self.prob.prob(front, back);
        if prob.is_infinite() {
            prob = 1.0;
        }
        let est_front = front - (1.0 - prob) * chg + (back - prob * chg).min(0.0);
        q.front_q_qty = est_front.min(new_qty);
    }

    fn is_filled(&self, order: &mut Order, depth: &MD) -> f64 {
        let q = order.q.as_any_mut().downcast_mut::<QueuePos>().unwrap();
        let exec = (-q.front_q_qty / depth.lot_size()).round() as i64;
        if exec > 0 {
            q.front_q_qty = 0.0;
            exec as f64 * depth.lot_size()
        } else {
            0.0
        }
    }
}

pub struct PowerProbQueueFunc {
    n: f64,
}
impl PowerProbQueueFunc {
    pub fn new(n: f64) -> Self {
        Self { n }
    }
    fn f(&self, x: f64) -> f64 {
        x.powf(self.n)
    }
}
impl Probability for PowerProbQueueFunc {
    fn prob(&self, front: f64, back: f64) -> f64 {
        self.f(back) / (self.f(back) + self.f(front))
    }
}

#[derive(Default)]
pub struct LogProbQueueFunc(());
impl LogProbQueueFunc {
    pub fn new() -> Self {
        Self::default()
    }
    fn f(&self, x: f64) -> f64 {
        (1.0 + x).ln()
    }
}
impl Probability for LogProbQueueFunc {
    fn prob(&self, front: f64, back: f64) -> f64 {
        self.f(back) / (self.f(back) + self.f(front))
    }
}

#[derive(Default)]
pub struct LogProbQueueFunc2(());
impl LogProbQueueFunc2 {
    pub fn new() -> Self {
        Self::default()
    }
    fn f(&self, x: f64) -> f64 {
        (1.0 + x).ln()
    }
}
impl Probability for LogProbQueueFunc2 {
    fn prob(&self, front: f64, back: f64) -> f64 {
        self.f(back) / self.f(back + front)
    }
}

pub struct PowerProbQueueFunc2 {
    n: f64,
}
impl PowerProbQueueFunc2 {
    pub fn new(n: f64) -> Self {
        Self { n }
    }
    fn f(&self, x: f64) -> f64 {
        x.powf(self.n)
    }
}
impl Probability for PowerProbQueueFunc2 {
    fn prob(&self, front: f64, back: f64) -> f64 {
        self.f(back) / self.f(back + front)
    }
}

pub struct PowerProbQueueFunc3 {
    n: f64,
}
impl PowerProbQueueFunc3 {
    pub fn new(n: f64) -> Self {
        Self { n }
    }
    fn f(&self, x: f64) -> f64 {
        x.powf(self.n)
    }
}
impl Probability for PowerProbQueueFunc3 {
    fn prob(&self, front: f64, back: f64) -> f64 {
        1.0 - self.f(front / (front + back))
    }
}
