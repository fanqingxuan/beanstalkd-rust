use crate::client::Connection;
use crate::commands::dispatch;
use crate::error::{CliError, Result};
use crate::usage::usage;
use rustyline::completion::{Completer, Pair};
use rustyline::config::CompletionType;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::history::DefaultHistory;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Config, Context, Editor, Helper};
use std::borrow::Cow;
use std::io::{self, IsTerminal, Write};

const COMMANDS: &[&str] = &[
    "put",
    "reserve",
    "delete",
    "release",
    "bury",
    "touch",
    "peek",
    "peek-ready",
    "peek-delayed",
    "peek-buried",
    "kick",
    "kick-job",
    "stats",
    "tubes",
    "list-tubes",
    "using",
    "list-tube-used",
    "watching",
    "list-tubes-watched",
    "pause-tube",
    "raw",
    "help",
    "exit",
    "quit",
];

const PUT_OPTIONS: &[&str] = &[
    "--tube",
    "-t",
    "--pri",
    "--priority",
    "--delay",
    "--ttr",
    "--file",
    "-f",
    "--stdin",
];

const RESERVE_OPTIONS: &[&str] = &["--timeout", "--watch", "-w", "--delete"];

pub(crate) fn run_repl(conn: &mut Connection) -> Result<()> {
    if io::stdin().is_terminal() {
        return run_interactive_repl(conn);
    }
    run_plain_repl(conn)
}

fn run_interactive_repl(conn: &mut Connection) -> Result<()> {
    let config = Config::builder()
        .completion_type(CompletionType::Circular)
        .build();
    let mut editor = Editor::<BeanstalkHelper, DefaultHistory>::with_config(config)
        .map_err(|err| CliError::new(err.to_string()))?;
    editor.set_helper(Some(BeanstalkHelper::new()));

    loop {
        match editor.readline("beanstalkctl> ") {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(input);
                if handle_repl_input(conn, input)? {
                    return Ok(());
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!();
                return Ok(());
            }
            Err(err) => return Err(CliError::new(err.to_string())),
        }
    }
}

fn run_plain_repl(conn: &mut Connection) -> Result<()> {
    let stdin = io::stdin();
    let mut line = String::new();

    loop {
        print!("beanstalkctl> ");
        io::stdout().flush()?;

        line.clear();
        let n = stdin.read_line(&mut line)?;
        if n == 0 {
            println!();
            return Ok(());
        }

        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if handle_repl_input(conn, input)? {
            return Ok(());
        }
    }
}

fn handle_repl_input(conn: &mut Connection, input: &str) -> Result<bool> {
    if matches!(input, "exit" | "quit") {
        return Ok(true);
    }
    if input == "help" {
        usage();
        return Ok(false);
    }
    match split_words(input).and_then(|mut words| {
        if words.is_empty() {
            return Ok(());
        }
        let command = words.remove(0);
        dispatch(conn, &command, words)
    }) {
        Ok(()) => {}
        Err(err) => eprintln!("error: {err}"),
    }
    Ok(false)
}

fn split_words(input: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }

    if escaped {
        current.push('\\');
    }
    if quote.is_some() {
        return Err(CliError::new("unterminated quote"));
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

struct BeanstalkHelper {
    hinter: HistoryHinter,
}

impl BeanstalkHelper {
    fn new() -> Self {
        Self {
            hinter: HistoryHinter {},
        }
    }
}

impl Helper for BeanstalkHelper {}

impl Completer for BeanstalkHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        Ok(completion_candidates(line, pos))
    }
}

fn completion_candidates(line: &str, pos: usize) -> (usize, Vec<Pair>) {
    let safe_pos = pos.min(line.len());
    let start = line[..safe_pos]
        .rfind(char::is_whitespace)
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let prefix = &line[start..safe_pos];
    let before = line[..start].trim_start();
    let command = before.split_whitespace().next();
    let candidates = match command {
        None => COMMANDS,
        Some("put") if prefix.starts_with('-') => PUT_OPTIONS,
        Some("reserve") if prefix.starts_with('-') => RESERVE_OPTIONS,
        Some("stats") => &["job", "tube"][..],
        _ if start == 0 => COMMANDS,
        _ => &[][..],
    };
    let matches = candidates
        .iter()
        .filter(|candidate| candidate.starts_with(prefix))
        .map(|candidate| completion_pair(candidate, line, safe_pos))
        .collect();
    (start, matches)
}

fn completion_pair(candidate: &str, line: &str, pos: usize) -> Pair {
    let has_space_after_cursor = line[pos..]
        .chars()
        .next()
        .map(|ch| ch.is_whitespace())
        .unwrap_or(false);
    let replacement = if has_space_after_cursor {
        candidate.to_string()
    } else {
        format!("{candidate} ")
    };
    Pair {
        display: candidate.to_string(),
        replacement,
    }
}

impl Hinter for BeanstalkHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        self.hinter.hint(line, pos, ctx)
    }
}

impl Highlighter for BeanstalkHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }
}

impl Validator for BeanstalkHelper {
    fn validate(&self, ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
        match split_words(ctx.input()) {
            Ok(_) => Ok(ValidationResult::Valid(None)),
            Err(err) => Ok(ValidationResult::Invalid(Some(err.to_string()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{completion_candidates, split_words};

    #[test]
    fn splits_repl_input_with_quotes() {
        assert_eq!(
            split_words("put --tube emails \"hello world\"").unwrap(),
            vec!["put", "--tube", "emails", "hello world"]
        );
    }

    #[test]
    fn completes_whole_commands_instead_of_common_prefixes() {
        let (start, matches) = completion_candidates("pe", 2);
        assert_eq!(start, 0);
        assert_eq!(matches[0].replacement, "peek ");
        assert_eq!(matches[1].replacement, "peek-ready ");
        assert!(!matches.iter().any(|item| item.replacement == "peek-"));
    }

    #[test]
    fn completes_whole_options() {
        let (start, matches) = completion_candidates("put --t", 7);
        assert_eq!(start, 4);
        assert_eq!(matches[0].replacement, "--tube ");
        assert!(matches.iter().any(|item| item.replacement == "--ttr "));
    }
}
