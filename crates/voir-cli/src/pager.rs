//! A minimal full-screen viewer for tables larger than the terminal: arrow
//! keys scroll, the header rows stay pinned, `q`/Esc leaves.

use std::io::{self, Write};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use crossterm::{execute, queue};

/// Lines that stay on screen while scrolling: top border, header, separator.
const PINNED: usize = 3;

/// Shows `text` in the viewer if it doesn't fit the terminal. Returns
/// `Ok(false)` — having drawn nothing — when it fits, so the caller prints
/// it normally.
pub fn show_if_needed(text: &str) -> io::Result<bool> {
    let (cols, rows) = terminal::size()?;
    let lines: Vec<&str> = text.lines().collect();
    let width = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    if lines.len() < usize::from(rows) && width <= usize::from(cols) {
        return Ok(false);
    }
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    let result = run(&lines, width);
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    result.map(|()| true)
}

fn run(lines: &[&str], width: usize) -> io::Result<()> {
    let pinned = PINNED.min(lines.len());
    let (head, body) = lines.split_at(pinned);
    let (mut top, mut left) = (0usize, 0usize);
    loop {
        let (cols, rows) = terminal::size()?;
        let (cols, rows) = (usize::from(cols), usize::from(rows));
        // Last row is the status line.
        let view = rows.saturating_sub(pinned + 1).max(1);
        let max_top = body.len().saturating_sub(view);
        let max_left = width.saturating_sub(cols);
        top = top.min(max_top);
        left = left.min(max_left);
        draw(head, body, top, left, view, cols, rows, width)?;

        let Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = event::read()?
        else {
            continue; // resize and the like: redraw
        };
        if kind == KeyEventKind::Release {
            continue;
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
            KeyCode::Up | KeyCode::Char('k') => top = top.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter => top += 1,
            KeyCode::PageUp => top = top.saturating_sub(view),
            KeyCode::PageDown | KeyCode::Char(' ') => top += view,
            KeyCode::Left | KeyCode::Char('h') => left = left.saturating_sub(4),
            KeyCode::Right | KeyCode::Char('l') => left += 4,
            KeyCode::Home | KeyCode::Char('g') => {
                top = 0;
                left = 0;
            }
            KeyCode::End | KeyCode::Char('G') => top = max_top,
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    head: &[&str],
    body: &[&str],
    top: usize,
    left: usize,
    view: usize,
    cols: usize,
    rows: usize,
    width: usize,
) -> io::Result<()> {
    let mut out = io::stdout().lock();
    queue!(out, Clear(ClearType::All))?;
    for (y, line) in (0u16..).zip(head.iter().chain(body.iter().skip(top).take(view))) {
        queue!(out, MoveTo(0, y))?;
        write!(out, "{}", slice_columns(line, left, cols))?;
    }
    // Body lines alternate data row / separator, ending in the bottom border.
    let total = body.len() / 2;
    let first = (top / 2 + 1).min(total);
    let last = ((top + view).min(body.len()) / 2).clamp(first, total);
    let status = format!(
        " rows {first}-{last} of {total} | cols {}-{} of {width} | arrows scroll, q quits ",
        left + 1,
        (left + cols).min(width),
    );
    let y = u16::try_from(rows.saturating_sub(1)).unwrap_or(u16::MAX);
    queue!(out, MoveTo(0, y))?;
    write!(out, "\x1b[7m{}\x1b[0m", slice_columns(&status, 0, cols))?;
    out.flush()
}

/// Number of terminal columns `s` occupies, ignoring ANSI escape sequences.
fn visible_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            skip_escape(&mut chars);
        } else {
            width += 1;
        }
    }
    width
}

fn skip_escape(chars: &mut std::str::Chars<'_>) {
    // CSI: ESC [ params final-byte(0x40..=0x7e)
    if chars.next() == Some('[') {
        for c in chars.by_ref() {
            if ('\u{40}'..='\u{7e}').contains(&c) {
                break;
            }
        }
    }
}

/// The part of `line` in visible columns `start..start + width`, keeping
/// every escape sequence so colors survive the cut, and resetting at the end.
fn slice_columns(line: &str, start: usize, width: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            if chars.peek() == Some(&'[') {
                out.push(chars.next().unwrap_or('['));
                for c in chars.by_ref() {
                    out.push(c);
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            if col >= start && col < start + width {
                out.push(c);
            }
            col += 1;
        }
    }
    out.push_str("\x1b[0m");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_ignores_escapes() {
        assert_eq!(visible_width("\x1b[1;94mabc\x1b[0m def"), 7);
    }

    #[test]
    fn slice_keeps_colors_and_cuts_columns() {
        let sliced = slice_columns("\x1b[31mabcdef\x1b[0m", 2, 3);
        assert_eq!(visible_width(&sliced), 3);
        assert!(sliced.starts_with("\x1b[31m") && sliced.contains("cde"));
    }
}
