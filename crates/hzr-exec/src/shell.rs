use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "safety", rename_all = "snake_case")]
pub enum ShellSafety {
    Simple { argv: Vec<String> },
    RawRequired { reason: String },
}

#[must_use]
pub fn analyze_shell(command: &str) -> ShellSafety {
    match parse_simple_shell(command) {
        Ok(argv) if argv.is_empty() => ShellSafety::RawRequired {
            reason: "empty shell command".to_owned(),
        },
        Ok(argv) if launches_nested_command(&argv) => ShellSafety::RawRequired {
            reason: "nested command launcher requires raw execution".to_owned(),
        },
        Ok(argv) => ShellSafety::Simple { argv },
        Err(reason) => ShellSafety::RawRequired { reason },
    }
}

pub fn parse_simple_shell(command: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = Quote::None;
    let mut started = false;

    while let Some(ch) = chars.next() {
        match quote {
            Quote::Single => match ch {
                '\'' => quote = Quote::None,
                _ => current.push(ch),
            },
            Quote::Double => match ch {
                '"' => quote = Quote::None,
                '\\' => {
                    let escaped = chars
                        .next()
                        .ok_or_else(|| "trailing escape in double quotes".to_owned())?;
                    if matches!(escaped, '$' | '`' | '"' | '\\') {
                        current.push(escaped);
                    } else if escaped == '\n' {
                        return Err("shell line continuation requires raw execution".to_owned());
                    } else {
                        current.push('\\');
                        current.push(escaped);
                    }
                }
                '`' => return Err("command substitution requires raw execution".to_owned()),
                '$' => return Err("shell expansion requires raw execution".to_owned()),
                _ => current.push(ch),
            },
            Quote::None => match ch {
                '\'' => {
                    quote = Quote::Single;
                    started = true;
                }
                '"' => {
                    quote = Quote::Double;
                    started = true;
                }
                '\\' => {
                    let escaped = chars.next().ok_or_else(|| "trailing escape".to_owned())?;
                    if escaped == '\n' {
                        return Err("shell line continuation requires raw execution".to_owned());
                    }
                    current.push(escaped);
                    started = true;
                }
                '`' => return Err("command substitution requires raw execution".to_owned()),
                '$' | '*' | '?' | '[' | ']' | '{' | '}' | '~' => {
                    return Err("shell expansion requires raw execution".to_owned());
                }
                '#' => return Err("shell comment syntax requires raw execution".to_owned()),
                '|' | '&' | ';' | '<' | '>' | '\n' | '\r' => {
                    return Err("shell control operator requires raw execution".to_owned());
                }
                c if c.is_whitespace() => {
                    if started {
                        args.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                _ => {
                    current.push(ch);
                    started = true;
                }
            },
        }
    }

    if quote != Quote::None {
        return Err("unterminated shell quote".to_owned());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

fn launches_nested_command(argv: &[String]) -> bool {
    argv.first().is_some_and(|word| word.contains('='))
        || matches!(
            argv.first().map(String::as_str),
            Some(
                "xargs"
                    | "eval"
                    | "sh"
                    | "bash"
                    | "zsh"
                    | "fish"
                    | "cmd"
                    | "powershell"
                    | "cd"
                    | "export"
                    | "source"
                    | "."
                    | "exec"
                    | "command"
                    | "builtin"
                    | "time"
                    | "if"
                    | "for"
                    | "while"
                    | "until"
                    | "case"
            )
        )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Quote {
    None,
    Single,
    Double,
}

#[cfg(test)]
mod tests {
    use super::{ShellSafety, analyze_shell, parse_simple_shell};

    #[test]
    fn test_parse_simple_shell_preserves_quoted_arguments() {
        assert_eq!(
            parse_simple_shell("git log --format='%h %s' \"two words\""),
            Ok(vec![
                "git".to_owned(),
                "log".to_owned(),
                "--format=%h %s".to_owned(),
                "two words".to_owned()
            ])
        );
    }

    #[test]
    fn test_analyze_shell_rejects_pipe() {
        assert!(matches!(
            analyze_shell("git log | head"),
            ShellSafety::RawRequired { .. }
        ));
    }

    #[test]
    fn test_analyze_shell_rejects_and_or_chains() {
        for command in ["cargo test && git push", "cargo test || true"] {
            assert!(matches!(
                analyze_shell(command),
                ShellSafety::RawRequired { .. }
            ));
        }
    }

    #[test]
    fn test_analyze_shell_rejects_redirects_and_heredoc() {
        for command in ["git status >out", "cat <in", "cat <<EOF\nbody\nEOF"] {
            assert!(matches!(
                analyze_shell(command),
                ShellSafety::RawRequired { .. }
            ));
        }
    }

    #[test]
    fn test_analyze_shell_rejects_xargs() {
        assert!(matches!(
            analyze_shell("xargs -n1 echo"),
            ShellSafety::RawRequired { .. }
        ));
    }

    #[test]
    fn test_parse_simple_shell_keeps_literal_operators_inside_quotes() {
        assert_eq!(
            parse_simple_shell("printf '%s' 'a|b && c'"),
            Ok(vec![
                "printf".to_owned(),
                "%s".to_owned(),
                "a|b && c".to_owned()
            ])
        );
    }
}
