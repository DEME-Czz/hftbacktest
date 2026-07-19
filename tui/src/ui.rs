use std::time::Instant;

use hftbacktest::types::{Side, Status};
use hftbacktest_tui::{AppState, Health};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};

pub fn draw(frame: &mut Frame, connector: &str, app: &AppState) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(frame, rows[0], connector, app);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(8)])
        .split(columns[0]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(8)])
        .split(columns[1]);

    draw_market(frame, left[0], app);
    draw_depth_and_trades(frame, left[1], app);
    draw_position(frame, right[0], app);
    draw_orders_and_events(frame, right[1], app);
    frame.render_widget(
        Paragraph::new("[p] Pause/Resume   [q] Quit   read-only: no order requests are sent")
            .style(Style::default().fg(Color::DarkGray)),
        rows[2],
    );
}

fn draw_header(frame: &mut Frame, area: Rect, connector: &str, app: &AppState) {
    let health = app.health_at(Instant::now());
    let color = match health {
        Health::Active => Color::Green,
        Health::Waiting | Health::Stale => Color::Yellow,
        Health::Disconnected | Health::Critical => Color::Red,
    };
    let pause = if app.paused() { " PAUSED" } else { "" };
    let line = Line::from(vec![
        Span::styled(
            " HftBacktest Live ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{connector} · {} · ", app.symbol().to_uppercase())),
        Span::styled(
            format!("{health:?}{pause}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · READ ONLY ", Style::default().fg(Color::Cyan)),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_market(frame: &mut Frame, area: Rect, app: &AppState) {
    let bid = format_level(app.best_bid());
    let ask = format_level(app.best_ask());
    let spread = match (app.best_bid(), app.best_ask()) {
        (Some((bid, _)), Some((ask, _))) => format!("{:.8}", ask - bid),
        _ => "unavailable".into(),
    };
    let feed_lag = app
        .last_feed_latency_ns()
        .map(format_ns)
        .unwrap_or_else(|| "unavailable".into());
    let text =
        format!("Best ask  {ask}\nSpread    {spread}\nBest bid  {bid}\nFeed lag  {feed_lag}");
    frame.render_widget(
        Paragraph::new(text).block(Block::default().title(" Market ").borders(Borders::ALL)),
        area,
    );
}

fn draw_position(frame: &mut Frame, area: Rect, app: &AppState) {
    let position = app
        .position()
        .map(|v| format!("{v:+}"))
        .unwrap_or_else(|| "unavailable".into());
    let order_lag = app
        .last_order_latency_ns()
        .map(format_ns)
        .unwrap_or_else(|| "unavailable".into());
    let text = format!(
        "Net qty       {position}\nOrder lag     {order_lag}\nWallet/PnL    unavailable\nTick / lot    {} / {}",
        app.tick_size(),
        app.lot_size()
    );
    frame.render_widget(
        Paragraph::new(text).block(Block::default().title(" Position ").borders(Borders::ALL)),
        area,
    );
}

fn draw_depth_and_trades(frame: &mut Frame, area: Rect, app: &AppState) {
    if area.height < 14 {
        draw_depth(frame, area, app);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);
    draw_depth(frame, rows[0], app);
    let trades = app
        .recent_trades()
        .iter()
        .rev()
        .take(rows[1].height.saturating_sub(2) as usize)
        .map(|trade| {
            let (side, color) = if trade.is(hftbacktest::types::LOCAL_BUY_TRADE_EVENT) {
                ("BUY ", Color::Green)
            } else {
                ("SELL", Color::Red)
            };
            Line::styled(
                format!("{side}  {:.8}  {:>12.4}", trade.px, trade.qty),
                Style::default().fg(color),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(trades).block(
            Block::default()
                .title(" Recent Trades ")
                .borders(Borders::ALL),
        ),
        rows[1],
    );
}

fn draw_depth(frame: &mut Frame, area: Rect, app: &AppState) {
    let rows = app
        .ask_levels(5)
        .into_iter()
        .rev()
        .map(|(px, qty)| {
            Line::styled(
                format!("ASK  {px:.8}  {qty:>12.4}"),
                Style::default().fg(Color::Red),
            )
        })
        .chain(app.bid_levels(5).into_iter().map(|(px, qty)| {
            Line::styled(
                format!("BID  {px:.8}  {qty:>12.4}"),
                Style::default().fg(Color::Green),
            )
        }))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(rows).block(Block::default().title(" Order Book ").borders(Borders::ALL)),
        area,
    );
}

fn draw_orders_and_events(frame: &mut Frame, area: Rect, app: &AppState) {
    if area.height < 14 {
        draw_orders(frame, area, app);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);
    draw_orders(frame, rows[0], app);
    let lines = app
        .events()
        .iter()
        .rev()
        .take(rows[1].height.saturating_sub(2) as usize)
        .map(|item| Line::raw(item.as_str()))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .title(" Events / Errors ")
                .borders(Borders::ALL),
        ),
        rows[1],
    );
}

fn draw_orders(frame: &mut Frame, area: Rect, app: &AppState) {
    let mut orders = app.orders().values().collect::<Vec<_>>();
    orders.sort_by_key(|order| std::cmp::Reverse(order.local_timestamp));
    let rows = orders
        .into_iter()
        .take(area.height.saturating_sub(3) as usize)
        .map(|order| {
            let color = match order.side {
                Side::Buy => Color::Green,
                Side::Sell => Color::Red,
                _ => Color::Gray,
            };
            Row::new(vec![
                Cell::from(order.order_id.to_string()),
                Cell::from(format!("{:?}", order.side)),
                Cell::from(format!("{:.8}", order.price())),
                Cell::from(format!("{}/{}", order.leaves_qty, order.qty)),
                Cell::from(status_name(order.status)),
            ])
            .style(Style::default().fg(color))
        });
    let widths = [
        Constraint::Length(9),
        Constraint::Length(5),
        Constraint::Length(12),
        Constraint::Length(13),
        Constraint::Min(8),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(Row::new(["ID", "Side", "Price", "Left/Qty", "Status"]))
            .block(Block::default().title(" Orders ").borders(Borders::ALL)),
        area,
    );
}

fn format_level(level: Option<(f64, f64)>) -> String {
    level
        .map(|(px, qty)| format!("{px:.8} x {qty}"))
        .unwrap_or_else(|| "unavailable".into())
}

fn format_ns(value: i64) -> String {
    if value.abs() >= 1_000_000 {
        format!("{:.2} ms", value as f64 / 1_000_000.0)
    } else if value.abs() >= 1_000 {
        format!("{:.2} µs", value as f64 / 1_000.0)
    } else {
        format!("{value} ns")
    }
}

fn status_name(status: Status) -> String {
    format!("{status:?}")
}
