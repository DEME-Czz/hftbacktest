mod config;
mod ui;

use std::{
    io::{self, Stdout},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use config::TuiConfig;
use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hftbacktest::{live::ipc::iceoryx::IceoryxBuilder, types::LiveEvent};
use hftbacktest_tui::AppState;
use ratatui::{Terminal, backend::CrosstermBackend};

#[derive(Debug, Parser)]
#[command(
    name = "hftbacktest-tui",
    about = "Read-only live monitor for HftBacktest connectors"
)]
struct Args {
    /// Path to the TOML file containing every TUI parameter.
    config: String,
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error).context("failed to enter alternate screen");
        }
        let terminal =
            Terminal::new(CrosstermBackend::new(stdout)).context("failed to create terminal")?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = TuiConfig::load(&args.config)?;

    // This creates only a ToBot subscriber. It never opens FromBot and therefore cannot submit a
    // RegisterInstrument or Order request.
    let receiver = IceoryxBuilder::new(&config.connector_name)
        .receiver::<LiveEvent>()
        .context("failed to subscribe to connector IPC; start the connector first")?;
    let mut app = AppState::new(
        &config.symbol,
        config.tick_size,
        config.lot_size,
        config.history_capacity,
    );
    let mut terminal = TerminalGuard::enter()?;

    loop {
        while let Some((_destination, live_event)) =
            receiver.receive().context(
                "IPC receive failed; connector and TUI may use different LiveEvent protocol versions—rebuild and restart both binaries",
            )?
        {
            app.apply(live_event);
        }
        terminal
            .terminal
            .draw(|frame| ui::draw(frame, &config.connector_name, &app))?;

        if event::poll(Duration::from_millis(config.poll_interval_ms))?
            && let TerminalEvent::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('p') => app.toggle_paused(),
                _ => {}
            }
        }
    }
    Ok(())
}
