use std::time::{Duration, Instant};

use chrono::Local;
use hftbacktest::types::{Side, Status};
use hftbacktest_tui::{AppState, Health, PositionDirection};
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
        .constraints([Constraint::Length(9), Constraint::Min(8)])
        .split(columns[0]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Min(8),
        ])
        .split(columns[1]);

    draw_market(frame, left[0], app);
    draw_depth_and_trades(frame, left[1], app);
    draw_position(frame, right[0], app);
    draw_performance(frame, right[1], app);
    draw_orders_and_events(frame, right[2], app);
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
        Span::raw(format!(" · {}", Local::now().format("%H:%M:%S"))),
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
        (Some((bid, _)), Some((ask, _))) => format!(
            "{:.8} / {}",
            ask - bid,
            app.spread_bps()
                .map(|value| format!("{value:.2} bp"))
                .unwrap_or_else(|| "unavailable".into())
        ),
        _ => "unavailable".into(),
    };
    let mid = app
        .mid_price()
        .map(|value| format!("{value:.8}"))
        .unwrap_or_else(|| "unavailable".into());
    let feed_lag = app
        .last_feed_latency_ns()
        .map(format_ns)
        .unwrap_or_else(|| "unavailable".into());
    let text = format!(
        "Best ask    {ask}\nSpread      {spread}\nMid price   {mid}\nBest bid    {bid}\nFeed lag    {feed_lag}\nTick / lot  {} / {}",
        app.tick_size(),
        app.lot_size()
    );
    frame.render_widget(
        Paragraph::new(text).block(Block::default().title(" Market ").borders(Borders::ALL)),
        area,
    );
}

fn draw_position(frame: &mut Frame, area: Rect, app: &AppState) {
    let (direction, direction_color) = match app.position_direction() {
        PositionDirection::Waiting => ("WAITING", Color::Yellow),
        PositionDirection::Flat => ("FLAT", Color::Gray),
        PositionDirection::Long => ("LONG", Color::Green),
        PositionDirection::Short => ("SHORT", Color::Red),
    };
    let position = app
        .position()
        .map(|v| format!("{v:+}"))
        .unwrap_or_else(|| "unavailable".into());
    let order_lag = app
        .last_order_latency_ns()
        .map(format_ns)
        .unwrap_or_else(|| "unavailable".into());
    let balance = app
        .balance()
        .map(|value| format!("{value:.8}"))
        .unwrap_or_else(|| "unavailable".into());
    let notional = app
        .position_notional()
        .map(|value| format!("{value:.4}"))
        .unwrap_or_else(|| "unavailable".into());
    let position_age = app
        .position_age(Instant::now())
        .map(format_age)
        .unwrap_or_else(|| "unavailable".into());
    let lines = vec![
        Line::from(vec![
            Span::raw("Direction       "),
            Span::styled(
                direction,
                Style::default()
                    .fg(direction_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(format!("Net quantity    {position}")),
        Line::raw(format!("Notional @ mid  {notional}")),
        Line::raw("Entry / mark     unavailable (protocol)"),
        Line::raw("Liquidation      unavailable (protocol)"),
        Line::raw(format!("Position age     {position_age}")),
        Line::raw(format!("Order lag        {order_lag}")),
        Line::raw(format!("Wallet           {balance}")),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" Position & Risk ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_performance(frame: &mut Frame, area: Rect, app: &AppState) {
    let wallet_change = app
        .wallet_change()
        .map(|value| format!("{value:+.8}"))
        .unwrap_or_else(|| "unavailable".into());
    let account_age = app
        .balance_age(Instant::now())
        .map(format_age)
        .unwrap_or_else(|| "unavailable".into());
    let text = format!(
        "Wallet change  {wallet_change}  (not PnL)\nRealized PnL   unavailable (protocol)\nUnrealized PnL unavailable (protocol)\nSession fees   -{:.8}\nFills / volume {} / {:.4}\nAccount age    {account_age}",
        app.fees(),
        app.num_fills(),
        app.filled_volume(),
    );
    frame.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .title(" Performance ")
                .borders(Borders::ALL),
        ),
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
    let (active, buys, sells) = app.active_order_counts();
    let mut orders = app
        .orders()
        .values()
        .filter(|order| order.active())
        .collect::<Vec<_>>();
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
            .block(
                Block::default()
                    .title(format!(
                        " Active Orders {active} · Buy {buys} · Sell {sells} "
                    ))
                    .borders(Borders::ALL),
            ),
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

fn format_age(value: Duration) -> String {
    if value.as_secs() >= 60 {
        format!("{}m {}s", value.as_secs() / 60, value.as_secs() % 60)
    } else if value.as_secs() > 0 {
        format!("{:.1}s", value.as_secs_f64())
    } else {
        format!("{}ms", value.as_millis())
    }
}

fn status_name(status: Status) -> String {
    format!("{status:?}")
}

#[cfg(test)]
mod tests {
    use hftbacktest_tui::AppState;
    use ratatui::{Terminal, backend::TestBackend};

    use super::draw;

    #[test]
    fn overview_renders_at_recommended_terminal_size() {
        render_at(120, 32);
    }

    #[test]
    fn overview_degrades_without_panicking_on_small_terminal() {
        render_at(80, 24);
    }

    fn render_at(width: u16, height: u16) {
        let app = AppState::new("dogeusdt", 0.00001, 1.0, 100);
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, "binancefutures-prod", &app))
            .unwrap();
    }
}
