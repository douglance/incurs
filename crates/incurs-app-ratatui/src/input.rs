//! Key handling.
//!
//! Every binding is a pure function from a key to a state change, so the whole
//! keyboard is testable by calling [`handle`] — no terminal, no event loop, no
//! timing.

use incurs_app_model::form::FieldKind;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, Focus};

/// Applies one key press.
pub fn handle(app: &mut App, key: KeyEvent) {
    // A terminal reports press and release on some platforms; act once.
    if key.kind == KeyEventKind::Release {
        return;
    }
    app.status = None;

    if app.help {
        app.help = false;
        return;
    }

    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match (control, key.code) {
        (true, KeyCode::Char('c')) => {
            // One key, two meanings, in the order a person expects: stop the
            // thing that is running, and if nothing is, leave.
            if app.session.is_running() {
                app.cancel();
            } else {
                app.quit = true;
            }
            return;
        }
        (true, KeyCode::Char('r')) => {
            app.run();
            return;
        }
        (true, KeyCode::Char('k')) => {
            app.install_skills();
            return;
        }
        _ => {}
    }

    if app.searching {
        search_key(app, key);
        return;
    }

    match key.code {
        KeyCode::Char('?') => app.help = true,
        KeyCode::Char('/') => {
            app.searching = true;
            app.focus = Focus::Commands;
        }
        KeyCode::Esc => {
            if !app.search.is_empty() {
                app.search.clear();
                app.select_row(0);
            } else {
                app.quit = true;
            }
        }
        KeyCode::Tab => app.focus = next_focus(app),
        KeyCode::BackTab => app.focus = previous_focus(app),
        _ => match app.focus {
            Focus::Commands => commands_key(app, key),
            Focus::Form => form_key(app, key),
        },
    }
}

/// Handles a key while the search box is active.
fn search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.searching = false;
            app.search.clear();
            app.select_row(0);
        }
        KeyCode::Enter => {
            app.searching = false;
            app.select_row(app.row);
        }
        KeyCode::Backspace => {
            app.search.backspace();
            app.select_row(0);
        }
        KeyCode::Char(character) => {
            app.search.insert(character);
            app.select_row(0);
        }
        _ => {}
    }
}

/// Handles a key in the command list.
fn commands_key(app: &mut App, key: KeyEvent) {
    let count = app.visible().len();
    if count == 0 {
        return;
    }
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => app.select_row((app.row + 1).min(count - 1)),
        KeyCode::Up | KeyCode::Char('k') => app.select_row(app.row.saturating_sub(1)),
        KeyCode::Home => app.select_row(0),
        KeyCode::End => app.select_row(count - 1),
        KeyCode::Enter => app.focus = Focus::Form,
        _ => {}
    }
}

/// Handles a key in the form.
fn form_key(app: &mut App, key: KeyEvent) {
    let count = app.field_count();
    if count == 0 {
        if key.code == KeyCode::Enter {
            app.run();
        }
        return;
    }

    let Some((name, kind)) = app.focused_field() else {
        return;
    };

    match (&kind, key.code) {
        (_, KeyCode::Down) => app.field = (app.field + 1).min(count - 1),
        (_, KeyCode::Up) => app.field = app.field.saturating_sub(1),

        (FieldKind::Toggle, KeyCode::Char(' ') | KeyCode::Enter) => {
            let value = app.session.state_mut().value_mut(&name);
            value.toggle = !value.toggle;
        }
        (FieldKind::Select { options }, code) => match code {
            KeyCode::Left => cycle_choice(app, &name, options, -1),
            KeyCode::Right | KeyCode::Char(' ') => cycle_choice(app, &name, options, 1),
            KeyCode::Enter => app.run(),
            _ => {}
        },

        // Every other kind is edited as text.
        (_, code) => {
            let Some(editor) = app.editors.get_mut(&name) else {
                return;
            };
            match code {
                KeyCode::Char(character) => editor.insert(character),
                KeyCode::Backspace => editor.backspace(),
                KeyCode::Delete => editor.delete(),
                KeyCode::Left => editor.left(),
                KeyCode::Right => editor.right(),
                KeyCode::Home => editor.home(),
                KeyCode::End => editor.end(),
                KeyCode::Enter => app.run(),
                _ => {}
            }
        }
    }
}

/// Moves one choice forward or backward, wrapping.
fn cycle_choice(app: &mut App, name: &str, options: &[String], step: isize) {
    if options.is_empty() {
        return;
    }
    let current = app
        .session
        .state()
        .value(name)
        .map(|value| value.choice.clone())
        .unwrap_or_default();
    let index = options.iter().position(|option| *option == current);
    let next = match index {
        Some(index) => {
            let len = options.len() as isize;
            (((index as isize + step) % len) + len) % len
        }
        // No choice yet: step forward lands on the first, back on the last.
        None if step > 0 => 0,
        None => options.len() as isize - 1,
    };
    app.session.state_mut().value_mut(name).choice = options[next as usize].clone();
}

/// Returns the pane Tab moves to.
fn next_focus(app: &App) -> Focus {
    match app.focus {
        Focus::Commands => Focus::Form,
        Focus::Form => Focus::Commands,
    }
}

/// Returns the pane Shift-Tab moves to.
fn previous_focus(app: &App) -> Focus {
    next_focus(app)
}
