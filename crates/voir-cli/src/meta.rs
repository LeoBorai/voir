use crate::session::Session;

/// Runs a `\`-prefixed meta-command. Returns `false` when the REPL should
/// exit (`\q`).
pub fn dispatch(cmd: &str, session: &mut Session) -> bool {
    let cmd = cmd.trim();
    let (name, rest) = cmd.split_once(char::is_whitespace).unwrap_or((cmd, ""));
    match name {
        "q" => return false,
        "?" => print_help(),
        "open" => session.cmd_open(rest.trim()),
        "dt" => session.cmd_list_tables(),
        "d" => session.cmd_describe(rest),
        "format" => session.cmd_format(rest),
        "timing" => session.cmd_timing(),
        "explain" => session.cmd_explain_toggle(),
        "i" => session.cmd_include(rest),
        "o" => session.cmd_output(rest),
        "save" => session.cmd_save(rest),
        "load" => session.cmd_load(rest),
        "autocomplete" => session.cmd_autocomplete(rest),
        "" => {}
        other => eprintln!("unknown command: \\{other} (try \\?)"),
    }
    true
}

fn print_help() {
    println!(
        "\\open <path> [AS <name>]  Register a file as a table\n\
         \\dt                       List registered tables\n\
         \\d <table>                Describe columns and inferred types\n\
         \\format <fmt>             Switch output format (table|csv|tsv|json|ndjson|markdown)\n\
         \\timing                   Toggle execution timing\n\
         \\explain                  Toggle plan printing\n\
         \\i <file>                 Execute a .sql file\n\
         \\o <file>                 Redirect results to a file (\\o to reset)\n\
         \\save <file>              Save registered tables and settings to a file\n\
         \\load <file>              Restore tables and settings from a \\save file\n\
         \\autocomplete on|off      Toggle the completion dropdown (default: on)\n\
         \\?                        Show this help\n\
         \\q                        Quit"
    );
}
