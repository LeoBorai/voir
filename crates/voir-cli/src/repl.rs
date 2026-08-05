use std::path::PathBuf;

use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config, Editor};

use crate::completion::VoirHelper;
use crate::diagnostics;
use crate::session::Session;
use crate::statement;

pub fn run(session: &mut Session) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::builder()
        .max_history_size(1000)?
        .history_ignore_dups(true)?
        .auto_add_history(false)
        .build();
    let mut editor: Editor<VoirHelper, DefaultHistory> = Editor::with_config(config)?;
    editor.set_helper(Some(VoirHelper::new()));

    let history_path = history_path();
    if let Some(path) = &history_path {
        let _ = editor.load_history(path);
    }

    println!("voir {} — type \\? for help", env!("CARGO_PKG_VERSION"));

    let mut buffer = String::new();
    let mut interrupted_once = false;
    loop {
        if let Some(helper) = editor.helper_mut() {
            helper.refresh(session);
        }
        let prompt = if buffer.is_empty() { "voir> " } else { "...> " };
        match editor.readline(prompt) {
            Ok(line) => {
                interrupted_once = false;
                if buffer.is_empty() {
                    if let Some(cmd) = line.trim_start().strip_prefix('\\') {
                        let _ = editor.add_history_entry(line.as_str());
                        if !crate::meta::dispatch(cmd.trim_end(), session) {
                            break;
                        }
                        continue;
                    }
                }
                if !buffer.is_empty() {
                    buffer.push('\n');
                }
                buffer.push_str(&line);
                if statement::is_complete(&buffer) {
                    let _ = editor.add_history_entry(buffer.as_str());
                    run_batch(session, &buffer);
                    buffer.clear();
                }
            }
            Err(ReadlineError::Interrupted) => {
                if buffer.is_empty() {
                    if interrupted_once {
                        break;
                    }
                    interrupted_once = true;
                    println!("(To exit, press Ctrl-C again or Ctrl-D)");
                } else {
                    buffer.clear();
                    interrupted_once = false;
                    println!();
                }
            }
            Err(ReadlineError::Eof) => {
                if buffer.is_empty() {
                    break;
                }
                buffer.clear();
                println!();
            }
            Err(err) => {
                eprintln!("error: {err}");
                break;
            }
        }
    }

    if let Some(path) = &history_path {
        let _ = editor.save_history(path);
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
