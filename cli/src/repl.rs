use crate::client::Connection;
use crate::commands::dispatch;
use crate::error::{CliError, Result};
use crate::usage::usage;
use std::io::{self, Write};

pub(crate) fn run_repl(conn: &mut Connection) -> Result<()> {
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
        if matches!(input, "exit" | "quit") {
            return Ok(());
        }
        if input == "help" {
            usage();
            continue;
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
    }
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

#[cfg(test)]
mod tests {
    use super::split_words;

    #[test]
    fn splits_repl_input_with_quotes() {
        assert_eq!(
            split_words("put --tube emails \"hello world\"").unwrap(),
            vec!["put", "--tube", "emails", "hello world"]
        );
    }
}
