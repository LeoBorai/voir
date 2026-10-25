use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nu_ansi_term::{Color, Style};
use reedline::{
    Completer, DescriptionMode, EditCommand, EditMode, Emacs, IdeMenu, KeyCode, KeyModifiers,
    Keybindings, MenuBuilder, PromptEditMode, ReedlineEvent, ReedlineRawEvent, Span, Suggestion,
    default_emacs_keybindings,
};

use crate::schema_cache::SchemaCache;
use crate::session::Session;
use crate::suggest::{self, Candidate, Context, Snapshot, TableSchema};

/// `reedline::Completer` adapter over `suggest.rs`: classifies, augments the
/// snapshot with an inferred file schema when the context names an
/// unregistered `FROM '<file>'` target, ranks, and shapes the result into
/// `Suggestion`s the `IdeMenu` can render.
pub struct VoirCompleter {
    snapshot: Arc<Mutex<Snapshot>>,
    schema_cache: SchemaCache,
}

impl VoirCompleter {
    #[must_use]
    pub fn new(snapshot: Arc<Mutex<Snapshot>>) -> Self {
        Self {
            snapshot,
            schema_cache: SchemaCache::default(),
        }
    }

    fn locked_snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Completer for VoirCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let context = suggest::classify(line, pos);
        match &context {
            Context::PathLiteral { prefix } | Context::MetaArgPath { prefix } => {
                crate::paths::complete(prefix, pos)
            }
            // Needs the snapshot augmented with an inferred file schema
            // before ranking, so it can't go through the one-shot
            // `suggest::candidates` below.
            Context::Expression { tables } => {
                let mut schema = self.locked_snapshot();
                for name in tables {
                    if schema.columns_of(name).is_some() {
                        continue;
                    }
                    if let Some(columns) = self.schema_cache.columns_for(Path::new(name)) {
                        schema.tables.push(TableSchema {
                            name: name.clone(),
                            columns,
                        });
                    }
                }
                let fragment = suggest::fragment(line, pos, &context);
                let word_start = pos.saturating_sub(fragment.len());
                suggest::rank(&context, &fragment, &schema)
                    .into_iter()
                    .map(|c| to_suggestion(c, word_start, pos))
                    .collect()
            }
            Context::StatementStart | Context::TableRef | Context::MetaCommand => {
                let schema = self.locked_snapshot();
                let fragment = suggest::fragment(line, pos, &context);
                let word_start = pos.saturating_sub(fragment.len());
                suggest::candidates(line, pos, &schema)
                    .into_iter()
                    .map(|c| to_suggestion(c, word_start, pos))
                    .collect()
            }
        }
    }
}

fn to_suggestion(candidate: Candidate, word_start: usize, pos: usize) -> Suggestion {
    Suggestion {
        value: candidate.value.clone(),
        display_override: Some(format!(
            "{:<24} {}",
            candidate.value,
            candidate.kind.label()
        )),
        description: candidate.source.map(|t| format!("column of {t}")),
        span: Span::new(word_start, pos),
        append_whitespace: false,
        ..Default::default()
    }
}

/// Snapshots the catalog's table/column names — the pre-`readline` refresh
/// `VoirHelper` used to do, now feeding the `Arc<Mutex<Snapshot>>` shared
/// with `VoirCompleter` since `reedline` owns the completer once installed.
#[must_use]
pub fn snapshot_of(session: &Session) -> Snapshot {
    let tables = session
        .catalog
        .tables()
        .map(|(name, table)| TableSchema {
            name: name.to_string(),
            columns: table
                .provider
                .schema()
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect(),
        })
        .collect();
    Snapshot { tables }
}

/// The dropdown itself — a borderless box under the cursor, selected row
/// inverted, per-row kind label from `display_override` padding since
/// `IdeMenu` only renders a description for the *selected* row.
#[must_use]
pub fn menu() -> IdeMenu {
    IdeMenu::default()
        .with_name("completion_menu")
        .with_max_completion_width(60)
        .with_max_completion_height(10)
        .with_padding(1)
        .with_text_style(Style::new().fg(Color::White).on(Color::DarkGray))
        .with_selected_text_style(Style::new().bold().fg(Color::Black).on(Color::LightCyan))
        .with_description_mode(DescriptionMode::PreferRight)
}

/// `default_emacs_keybindings()` plus the bindings the dropdown needs that
/// have no default: Tab opens the menu, or accepts the hovered row once open, Down/Up fall through to
/// history when it's closed, Alt-Enter inserts a literal newline. Enter and
/// Esc already do the right thing (accept-or-submit, dismiss) by default.
#[must_use]
pub fn keybindings() -> Keybindings {
    let mut kb = default_emacs_keybindings();
    kb.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".into()),
            // Menu already open: `Menu` is inapplicable, so Tab accepts
            // the hovered suggestion (Enter does the same while open).
            ReedlineEvent::Enter,
        ]),
    );
    kb.add_binding(
        KeyModifiers::NONE,
        KeyCode::Down,
        ReedlineEvent::UntilFound(vec![ReedlineEvent::MenuNext, ReedlineEvent::Down]),
    );
    kb.add_binding(
        KeyModifiers::NONE,
        KeyCode::Up,
        ReedlineEvent::UntilFound(vec![ReedlineEvent::MenuPrevious, ReedlineEvent::Up]),
    );
    kb.add_binding(
        KeyModifiers::ALT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
    kb
}

/// Wraps `Emacs` to open the completion menu on a bare word-character
/// insert, not just Tab — the difference between "Tab completion in a
/// nicer box" and a `pgcli`-style dropdown that follows every keystroke.
/// Gated on `enabled` (`\autocomplete on|off`) and on word characters only —
/// opening after a space or `(` would flash the full keyword list at every
/// token boundary.
pub struct AutoMenu {
    inner: Emacs,
    enabled: Arc<AtomicBool>,
}

impl AutoMenu {
    #[must_use]
    pub fn new(enabled: Arc<AtomicBool>) -> Self {
        Self {
            inner: Emacs::new(keybindings()),
            enabled,
        }
    }
}

impl EditMode for AutoMenu {
    fn parse_event(&mut self, event: ReedlineRawEvent) -> ReedlineEvent {
        let event = self.inner.parse_event(event);
        if self.enabled.load(Ordering::Relaxed) && inserts_word_char(&event) {
            ReedlineEvent::Multiple(vec![event, ReedlineEvent::Menu("completion_menu".into())])
        } else {
            event
        }
    }

    fn edit_mode(&self) -> PromptEditMode {
        self.inner.edit_mode()
    }
}

fn inserts_word_char(event: &ReedlineEvent) -> bool {
    match event {
        ReedlineEvent::Edit(cmds) => cmds.iter().any(
            |c| matches!(c, EditCommand::InsertChar(ch) if ch.is_alphanumeric() || *ch == '_'),
        ),
        _ => false,
    }
}
