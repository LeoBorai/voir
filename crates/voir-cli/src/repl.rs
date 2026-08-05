use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use reedline::{FileBackedHistory, History, Reedline, ReedlineMenu, Signal};

use crate::completion::{self, AutoMenu, VoirCompleter};
use crate::diagnostics;
use crate::highlight::VoirHighlighter;
use crate::prompt::VoirPrompt;
use crate::session::Session;
use crate::statement;
use crate::validator::VoirValidator;

pub fn run(session: &mut Session) -> Result<(), Box<dyn std::error::Error>> {
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();

    let history_path = history_path();
    let history: Box<dyn History> = match &history_path {
        Some(path) => Box::new(FileBackedHistory::with_file(1000, path.clone())?),
        None => Box::new(FileBackedHistory::new(1000)?),
    };

    let snapshot = Arc::new(Mutex::new(completion::snapshot_of(session)));
    let completer = VoirCompleter::new(snapshot.clone());
    let edit_mode = Box::new(AutoMenu::new(session.autocomplete.clone()));

    let mut editor = Reedline::create()
        .with_history(history)
        .with_completer(Box::new(completer))
        .with_highlighter(Box::new(VoirHighlighter { color }))
        .with_validator(Box::new(VoirValidator))
        .with_edit_mode(edit_mode)
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(completion::menu())))
        .with_quick_completions(true)
        .with_partial_completions(true);

    println!("voir {} — type \\? for help", env!("CARGO_PKG_VERSION"));

    let prompt = VoirPrompt;
    let mut interrupted_once = false;
    loop {
        // Refreshed ahead of every prompt — reedline owns the completer
        // once installed, so state can't borrow the session live; this
        // mirrors the old `VoirHelper::refresh` snapshot-before-readline.
        if let Ok(mut guard) = snapshot.lock() {
            *guard = completion::snapshot_of(session);
        }

        match editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                interrupted_once = false;
                if let Some(cmd) = buffer.trim_start().strip_prefix('\\') {
                    if !crate::meta::dispatch(cmd.trim_end(), session) {
                        break;
                    }
                    continue;
                }
                run_batch(session, &buffer);
            }
            Ok(Signal::CtrlC) => {
                if interrupted_once {
                    break;
                }
                interrupted_once = true;
                println!("(To exit, press Ctrl-C again or Ctrl-D)");
            }
            // CtrlD (session EOF) and any other signal (HostCommand,
            // ExternalBreak — unused here) all mean "stop the loop".
            Ok(_) => break,
            Err(err) => {
                eprintln!("error: {err}");
                break;
            }
        }
    }

    Ok(())
}

/// Runs every top-level `;`-separated statement in `buffer` in order,
/// stopping at the first failure — matching `Session::cmd_include`'s
/// behavior for `\i`. Fixes the REPL previously handing a whole multi-
/// statement buffer to the parser as one string.
fn run_batch(session: &mut Session, buffer: &str) {
    match statement::split(buffer) {
        Ok(statements) => {
            for stmt in statements {
                if !session.execute(&stmt) {
                    break;
                }
            }
        }
        Err(err) => diagnostics::report(buffer, &err),
    }
}

fn history_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    let dir = base.join("voir");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("history"))
}
