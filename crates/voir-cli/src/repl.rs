use std::path::PathBuf;

use rustyline::Editor;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;

use crate::completion::VoirHelper;
use crate::session::Session;
use crate::statement;

pub fn run(session: &mut Session) -> Result<(), Box<dyn std::error::Error>> {
    let mut editor: Editor<VoirHelper, DefaultHistory> = Editor::new()?;
    editor.set_helper(Some(VoirHelper::new()));

    let history_path = history_path();
    if let Some(path) = &history_path {
        let _ = editor.load_history(path);
    }

    let mut buffer = String::new();
    loop {
        if let Some(helper) = editor.helper_mut() {
            helper.refresh(session);
        }
        let prompt = if buffer.is_empty() { "voir> " } else { "...> " };
        match editor.readline(prompt) {
            Ok(line) => {
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
                    session.execute(&buffer);
                    buffer.clear();
                }
            }
            Err(ReadlineError::Interrupted) => {
                buffer.clear();
                println!();
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

fn history_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    let dir = base.join("voir");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("history"))
}
