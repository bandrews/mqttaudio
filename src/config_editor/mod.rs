// ABOUTME: Interactive terminal UI for creating and editing the mqttaudio config file.
// ABOUTME: Owns the terminal lifecycle and event loop; state lives in app, drawing in form.

pub mod app;
pub mod device_test;
pub mod fields;
pub mod form;
pub mod save;

use app::{App, Modal};
use ratatui::crossterm::event::{self, Event};
use save::{load_document, LoadOutcome};
use std::time::Duration;

/// Tick interval: drives meter refresh, sweep timing, and session polling.
const TICK: Duration = Duration::from_millis(50);

/// Run the interactive config editor. `config_path` is the --config argument:
/// it is loaded when given (or the daemon's search paths otherwise) and is the
/// default save target.
pub fn run(config_path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = match load_document(config_path) {
        LoadOutcome::Loaded { path, doc } => App::new(doc, Some(path)),
        LoadOutcome::NotFound => {
            let mut app = App::new(fields::ConfigDocument::new(), None);
            if let Some(p) = config_path {
                app.loaded_from = Some(std::path::PathBuf::from(p));
                app.status = format!("New configuration (will save to {})", p);
            }
            app
        }
        LoadOutcome::Failed { path, error } => {
            let mut app = App::new(fields::ConfigDocument::new(), Some(path.clone()));
            app.modal = Some(Modal::LoadFailed { path, error });
            app
        }
    };

    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    while !app.quit {
        terminal.draw(|frame| form::draw(frame, app))?;
        if event::poll(TICK)? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key);
            }
        }
        app.tick();
    }
    Ok(())
}
