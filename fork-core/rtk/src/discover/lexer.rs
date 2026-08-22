#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeKind {
    Stdout,
    StdoutAndStderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Arg,
    Operator,
    Pipe(PipeKind),
    Redirect,
    Shellism,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToken {
    pub kind: TokenKind,
    pub value: String,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandPayload {
    pub command: String,
    pub range: std::ops::Range<usize>,
    quote: Option<char>,
}

impl ShellCommandPayload {
    pub fn replace_in(&self, invocation: &str, rewritten: &str) -> String {
        let replacement = match self.quote {
            Some(quote) => format!("{quote}{rewritten}{quote}"),
            None => format!("'{}'", rewritten.replace('\'', "'\\''")),
        };
        format!(
            "{}{}{}",
            &invocation[..self.range.start],
            replacement,
            &invocation[self.range.end..]
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellWrapperParse {
    NotWrapper,
    Parsed(ShellCommandPayload),
    Ambiguous,
}

pub fn tokenize(input: &str) -> Vec<ParsedToken> {
    tokenize_inner(input, false)
}

pub fn tokenize_with_newlines(input: &str) -> Vec<ParsedToken> {
    tokenize_inner(input, true)
}

pub fn parse_shell_command_wrapper(input: &str) -> ShellWrapperParse {
    let tokens = tokenize(input);
    let Some(program) = tokens.first() else {
        return ShellWrapperParse::NotWrapper;
    };
    if program.kind != TokenKind::Arg || !is_shell_program(&program.value) {
        return ShellWrapperParse::NotWrapper;
    }
    if !quotes_balanced(input) || tokens.iter().any(|token| token.kind != TokenKind::Arg) {
        return ShellWrapperParse::Ambiguous;
    }

    let mut command_flag = None;
    let mut index = 1;
    while let Some(token) = tokens.get(index) {
        let Some(value) = unquote_word(&token.value) else {
            return ShellWrapperParse::Ambiguous;
        };
        if value.starts_with('-')
            && !value.starts_with("--")
            && value[1..].chars().any(|flag| flag == 'c')
        {
            command_flag = Some(index);
            break;
        }
        if matches!(value, "-O" | "+O" | "--rcfile" | "--init-file") {
            if tokens.get(index + 1).is_none() {
                return ShellWrapperParse::Ambiguous;
            }
            index += 2;
            continue;
        }
        if !value.starts_with(['-', '+']) || value == "--" {
            let later_command_flag = tokens.iter().skip(index + 1).any(|later| {
                unquote_word(&later.value).is_some_and(|word| {
                    word.starts_with('-')
                        && !word.starts_with("--")
                        && word[1..].chars().any(|flag| flag == 'c')
                })
            });
            return if later_command_flag {
                ShellWrapperParse::Ambiguous
            } else {
                ShellWrapperParse::NotWrapper
            };
        }
        index += 1;
    }
    let Some(command_flag) = command_flag else {
        return ShellWrapperParse::NotWrapper;
    };
    let Some(payload) = tokens.get(command_flag + 1) else {
        return ShellWrapperParse::Ambiguous;
    };
    let raw = &input[payload.offset..payload.offset + payload.value.len()];
    let (command, quote) = match (raw.chars().next(), raw.chars().last()) {
        (Some(first @ ('\'' | '"')), Some(last)) if first == last => (
            raw[first.len_utf8()..raw.len() - last.len_utf8()].to_owned(),
            Some(first),
        ),
        (Some('\'' | '"'), _) | (_, Some('\'' | '"')) => return ShellWrapperParse::Ambiguous,
        _ => (raw.to_owned(), None),
    };
    ShellWrapperParse::Parsed(ShellCommandPayload {
        command,
        range: payload.offset..payload.offset + payload.value.len(),
        quote,
    })
}

fn is_shell_program(value: &str) -> bool {
    let Some(value) = unquote_word(value) else {
        return false;
    };
    matches!(
        value.rsplit('/').next(),
        Some("sh" | "bash" | "dash" | "ksh" | "zsh")
    )
}

fn unquote_word(value: &str) -> Option<&str> {
    match (value.as_bytes().first(), value.as_bytes().last()) {
        (Some(first @ (b'\'' | b'"')), Some(last)) if first == last => {
            Some(&value[1..value.len() - 1])
        }
        (Some(b'\'' | b'"'), _) | (_, Some(b'\'' | b'"')) => None,
        _ => Some(value),
    }
}

fn quotes_balanced(input: &str) -> bool {
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for byte in input.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && !single {
            escaped = true;
        } else if byte == b'\'' && !double {
            single = !single;
        } else if byte == b'"' && !single {
            double = !double;
        }
    }
    !single && !double && !escaped
}

fn tokenize_inner(input: &str, emit_newline: bool) -> Vec<ParsedToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_start: usize = 0;
    let mut byte_pos: usize = 0;
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        let char_len = c.len_utf8();

        if escaped {
            current.push('\\');
            current.push(c);
            byte_pos += char_len;
            escaped = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            if current.is_empty() {
                current_start = byte_pos;
            }
            byte_pos += char_len;
            continue;
        }

        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            current.push(c);
            byte_pos += char_len;
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            if current.is_empty() {
                current_start = byte_pos;
            }
            current.push(c);
            byte_pos += char_len;
            continue;
        }

        match c {
            '$' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars
                    .peek()
                    .is_some_and(|&nc| nc.is_ascii_alphabetic() || nc == '_')
                {
                    let mut name = String::from("$");
                    while let Some(&nc) = chars.peek() {
                        if !nc.is_ascii_alphanumeric() && nc != '_' {
                            break;
                        }
                        chars.next();
                        byte_pos += nc.len_utf8();
                        name.push(nc);
                    }
                    tokens.push(ParsedToken {
                        kind: TokenKind::Arg,
                        value: name,
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Shellism,
                        value: "$".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            '*' | '?' | '`' | '(' | ')' | '{' | '}' | '!' => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Shellism,
                    value: c.to_string(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            '|' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars.peek() == Some(&'|') {
                    chars.next();
                    byte_pos += 1;
                    tokens.push(ParsedToken {
                        kind: TokenKind::Operator,
                        value: "||".into(),
                        offset: start,
                    });
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    tokens.push(ParsedToken {
                        kind: TokenKind::Pipe(PipeKind::StdoutAndStderr),
                        value: "|&".into(),
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Pipe(PipeKind::Stdout),
                        value: "|".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            ';' => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Operator,
                    value: ";".into(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            '&' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    tokens.push(ParsedToken {
                        kind: TokenKind::Operator,
                        value: "&&".into(),
                        offset: start,
                    });
                } else if chars.peek() == Some(&'>') {
                    chars.next();
                    byte_pos += 1;
                    let mut val = String::from("&>");
                    if chars.peek() == Some(&'>') {
                        chars.next();
                        byte_pos += 1;
                        val.push('>');
                    }
                    tokens.push(ParsedToken {
                        kind: TokenKind::Redirect,
                        value: val,
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Shellism,
                        value: "&".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            '>' => {
                let fd_prefix =
                    if !current.is_empty() && current.chars().all(|ch| ch.is_ascii_digit()) {
                        Some(std::mem::take(&mut current))
                    } else {
                        flush_arg(&mut tokens, &mut current, current_start);
                        None
                    };
                let redir_start = if fd_prefix.is_some() {
                    current_start
                } else {
                    byte_pos
                };
                let mut val = fd_prefix.unwrap_or_default();
                val.push('>');
                byte_pos += char_len;
                if chars.peek() == Some(&'>') {
                    chars.next();
                    byte_pos += 1;
                    val.push('>');
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    val.push('&');
                    while let Some(&nc) = chars.peek() {
                        if !nc.is_ascii_digit() && nc != '-' {
                            break;
                        }
                        chars.next();
                        val.push(nc);
                        byte_pos += nc.len_utf8();
                    }
                }
                tokens.push(ParsedToken {
                    kind: TokenKind::Redirect,
                    value: val,
                    offset: redir_start,
                });
                current_start = byte_pos;
            }
            '<' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                let mut val = String::from("<");
                byte_pos += char_len;
                if chars.peek() == Some(&'<') {
                    chars.next();
                    byte_pos += 1;
                    val.push('<');
                }
                tokens.push(ParsedToken {
                    kind: TokenKind::Redirect,
                    value: val,
                    offset: start,
                });
                current_start = byte_pos;
            }
            // `\n`, and the `\r` of a CRLF pair, are command separators. A LONE `\r`
            // (classic-Mac EOL, not followed by `\n`) is not: bash treats a bare CR as
            // an ordinary character inside a word, so `git status\rrm -rf ~` is one
            // mangled git command and never runs the `rm`. Emitting a separator for it
            // over-segmented the permission split and let the rewriter re-join the
            // pieces into something the permission layer never approved. A lone `\r`
            // now falls through to the whitespace arm — a word break at most.
            '\n' if emit_newline => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Operator,
                    value: "\n".into(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            '\r' if emit_newline && chars.peek() == Some(&'\n') => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Operator,
                    value: "\n".into(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            c if c.is_whitespace() => {
                flush_arg(&mut tokens, &mut current, current_start);
                byte_pos += c.len_utf8();
                current_start = byte_pos;
            }
            _ => {
                if current.is_empty() {
                    current_start = byte_pos;
                }
                current.push(c);
                byte_pos += char_len;
            }
        }
    }

    if escaped {
        current.push('\\');
    }
    flush_arg(&mut tokens, &mut current, current_start);
    tokens
}

fn flush_arg(tokens: &mut Vec<ParsedToken>, current: &mut String, offset: usize) {
    if !current.is_empty() {
        tokens.push(ParsedToken {
            kind: TokenKind::Arg,
            value: std::mem::take(current),
            offset,
        });
    }
}

/// True for constructs the permission gate can't decompose, so they must never
/// be auto-allowed: command/process substitution, or a real file-target redirect
/// (fd-dup like `2>&1` and `/dev/null` are exempt). Separators and subshells are
/// handled by [`split_for_permissions`], not flagged here.
pub fn contains_unattestable_construct(cmd: &str) -> bool {
    if contains_substitution(cmd) {
        return true;
    }
    let tokens = tokenize(cmd);
    tokens
        .iter()
        .enumerate()
        .any(|(i, tok)| tok.kind == TokenKind::Redirect && redirect_has_file_target(&tokens, i))
}

/// Quote-aware: bash runs backtick/`$(...)` unquoted and inside double quotes,
/// but treats single-quoted text literally; `<(`/`>(` is unquoted-only.
fn contains_substitution(cmd: &str) -> bool {
    let bytes = cmd.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if !in_single => {
                i += 2;
                continue;
            }
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'`' if !in_single => return true,
            b'$' if !in_single && bytes.get(i + 1) == Some(&b'(') => return true,
            b'<' | b'>' if !in_single && !in_double && bytes.get(i + 1) == Some(&b'(') => {
                return true
            }
            _ => {}
        }
        i += 1;
    }
    false
}

// `>&N`/`>&-` (and `N>&M`) is fd-dup/close; bare `>&` before a word is
// `>word 2>&1` — a file target.
fn redirect_has_file_target(tokens: &[ParsedToken], i: usize) -> bool {
    let value = &tokens[i].value;
    if let Some(pos) = value.find(">&") {
        let tail = &value[pos + 2..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return false;
        }
    }
    match tokens.get(i + 1) {
        Some(next) if next.kind == TokenKind::Arg => next.value != "/dev/null",
        _ => true,
    }
}

pub fn has_stdout_file_redirect(cmd: &str) -> bool {
    let tokens = tokenize(cmd);
    tokens.iter().enumerate().any(|(i, token)| {
        if token.kind != TokenKind::Redirect || !redirect_has_file_target(&tokens, i) {
            return false;
        }

        let redirect = token.value.as_str();
        redirect.starts_with('>') || redirect.starts_with("1>") || redirect.starts_with("&>")
    })
}

/// Like [`split_on_operators`] but also breaks on newline, background `&`, and
/// subshell `( ... )`, and truncates each segment at its first redirect.
/// Callers must still gate on [`contains_unattestable_construct`] first.
pub fn split_for_permissions(cmd: &str) -> Vec<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let tokens = tokenize_inner(trimmed, true);
    let mut results = Vec::new();
    let mut seg_start: usize = 0;
    let mut seg_end: Option<usize> = None;

    for tok in &tokens {
        let is_boundary = match tok.kind {
            TokenKind::Operator | TokenKind::Pipe(_) => true,
            TokenKind::Shellism => matches!(tok.value.as_str(), "&" | "(" | ")"),
            _ => false,
        };

        if is_boundary {
            let end = seg_end.take().unwrap_or(tok.offset);
            let segment = trimmed[seg_start..end].trim();
            if !segment.is_empty() {
                results.push(segment);
            }
            seg_start = tok.offset + tok.value.len();
        } else if tok.kind == TokenKind::Redirect && seg_end.is_none() {
            seg_end = Some(tok.offset);
        }
    }

    let end = seg_end.unwrap_or(trimmed.len());
    let tail = trimmed[seg_start..end].trim();
    if !tail.is_empty() {
        results.push(tail);
    }

    results
}

/// Split a shell command on operators (`&&`, `||`, `;`) and optionally pipes (`|`),
/// respecting quoted strings via the lexer.
///
/// When `stop_at_pipe` is true, returns only segments before the first `|`
/// (used by command rewriting — only the left side of a pipe gets rewritten).
/// When false, splits through pipes too (used by permission checking —
/// every segment must be validated).
pub fn split_on_operators(cmd: &str, stop_at_pipe: bool) -> Vec<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let tokens = tokenize(trimmed);
    let mut results = Vec::new();
    let mut seg_start: usize = 0;

    for tok in &tokens {
        match tok.kind {
            TokenKind::Operator => {
                let segment = trimmed[seg_start..tok.offset].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                seg_start = tok.offset + tok.value.len();
            }
            TokenKind::Pipe(_) => {
                let segment = trimmed[seg_start..tok.offset].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                if stop_at_pipe {
                    return results;
                }
                seg_start = tok.offset + tok.value.len();
            }
            _ => {}
        }
    }

    let tail = trimmed[seg_start..].trim();
    if !tail.is_empty() {
        results.push(tail);
    }

    results
}

#[cfg(test)]
pub fn strip_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() >= 2
        && ((chars[0] == '"' && chars[chars.len() - 1] == '"')
            || (chars[0] == '\'' && chars[chars.len() - 1] == '\''))
    {
        return chars[1..chars.len() - 1].iter().collect();
    }
    s.to_string()
}

pub fn shell_split(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_command() {
        let tokens = tokenize("git status");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].kind, TokenKind::Arg);
        assert_eq!(tokens[0].value, "git");
        assert_eq!(tokens[1].value, "status");
    }

    #[test]
    fn test_command_with_args() {
        let tokens = tokenize("git commit -m message");
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].value, "git");
        assert_eq!(tokens[1].value, "commit");
        assert_eq!(tokens[2].value, "-m");
        assert_eq!(tokens[3].value, "message");
    }

    #[test]
    fn test_quoted_operator_not_split() {
        let tokens = tokenize(r#"git commit -m "Fix && Bug""#);
        assert!(!tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Operator) && t.value == "&&"));
        assert!(tokens.iter().any(|t| t.value.contains("Fix && Bug")));
    }

    #[test]
    fn test_single_quoted_string() {
        let tokens = tokenize("echo 'hello world'");
        assert!(tokens.iter().any(|t| t.value == "'hello world'"));
    }

    #[test]
    fn test_double_quoted_string() {
        let tokens = tokenize(r#"echo "hello world""#);
        assert!(tokens.iter().any(|t| t.value == "\"hello world\""));
    }

    #[test]
    fn test_empty_quoted_string() {
        let tokens = tokenize("echo \"\"");
        assert!(tokens.iter().any(|t| t.value == "\"\""));
    }

    #[test]
    fn test_nested_quotes() {
        let tokens = tokenize(r#"echo "outer 'inner' outer""#);
        assert!(tokens.iter().any(|t| t.value.contains("'inner'")));
    }

    #[test]
    fn test_escaped_space() {
        let tokens = tokenize("echo hello\\ world");
        assert!(tokens.iter().any(|t| t.value.contains("hello")));
    }

    #[test]
    fn test_backslash_in_single_quotes() {
        let tokens = tokenize(r#"echo 'hello\nworld'"#);
        assert!(tokens.iter().any(|t| t.value.contains(r"\n")));
    }

    #[test]
    fn test_escaped_quote_in_double() {
        let tokens = tokenize(r#"echo "hello\"world""#);
        assert!(tokens.iter().any(|t| t.value.contains("hello")));
    }

    #[test]
    fn test_empty_input() {
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn test_whitespace_only() {
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn test_unclosed_single_quote() {
        let tokens = tokenize("'unclosed");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_unclosed_double_quote() {
        let tokens = tokenize("\"unclosed");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_unicode_preservation() {
        let tokens = tokenize("echo \"héllo wörld\"");
        assert!(tokens.iter().any(|t| t.value.contains("héllo")));
    }

    #[test]
    fn test_multiple_spaces() {
        let tokens = tokenize("git   status");
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_leading_trailing_spaces() {
        let tokens = tokenize("  git status  ");
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_and_operator() {
        let tokens = tokenize("cmd1 && cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "&&"));
    }

    #[test]
    fn test_or_operator() {
        let tokens = tokenize("cmd1 || cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "||"));
    }

    #[test]
    fn test_semicolon() {
        let tokens = tokenize("cmd1 ; cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == ";"));
    }

    #[test]
    fn test_multiple_and() {
        let tokens = tokenize("a && b && c");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_mixed_operators() {
        let tokens = tokenize("a && b || c");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_operator_at_start() {
        let tokens = tokenize("&& cmd");
        assert!(tokens.iter().any(|t| t.value == "&&"));
    }

    #[test]
    fn test_operator_at_end() {
        let tokens = tokenize("cmd &&");
        assert!(tokens.iter().any(|t| t.value == "&&"));
    }

    #[test]
    fn test_pipe_detection() {
        let tokens = tokenize("cat file | grep pattern");
        assert!(tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Pipe(PipeKind::Stdout))));
    }

    #[test]
    fn test_stderr_pipe_is_atomic() {
        let tokens = tokenize("cargo test |& grep FAILED");
        let pipes: Vec<_> = tokens
            .iter()
            .filter(|token| matches!(token.kind, TokenKind::Pipe(_)))
            .collect();

        assert_eq!(pipes.len(), 1);
        assert_eq!(pipes[0].kind, TokenKind::Pipe(PipeKind::StdoutAndStderr));
        assert_eq!(pipes[0].value, "|&");
        assert!(!tokens
            .iter()
            .any(|token| token.kind == TokenKind::Shellism && token.value == "&"));
        assert_eq!(
            split_for_permissions("cargo test |& grep FAILED"),
            vec!["cargo test", "grep FAILED"]
        );
    }

    #[test]
    fn test_quoted_pipe_not_pipe() {
        let tokens = tokenize("\"a|b\"");
        assert!(!tokens.iter().any(|t| matches!(t.kind, TokenKind::Pipe(_))));
    }

    #[test]
    fn test_multiple_pipes() {
        let tokens = tokenize("a | b | c");
        let pipes: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Pipe(_)))
            .collect();
        assert_eq!(pipes.len(), 2);
    }

    #[test]
    fn test_glob_detection() {
        let tokens = tokenize("ls *.rs");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_quoted_glob_not_shellism() {
        let tokens = tokenize("echo \"*.txt\"");
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_simple_var_is_arg() {
        let tokens = tokenize("echo $HOME");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Arg && t.value == "$HOME"),
            "Simple $VAR must be Arg — shell expands at execution time"
        );
        assert!(
            !tokens.iter().any(|t| t.kind == TokenKind::Shellism),
            "No Shellism expected for simple $VAR"
        );
    }

    #[test]
    fn test_simple_var_enables_native_routing() {
        let tokens = tokenize("git log $BRANCH");
        assert!(
            !tokens.iter().any(|t| t.kind == TokenKind::Shellism),
            "git log $BRANCH must have no Shellism"
        );
    }

    #[test]
    fn test_dollar_subshell_stays_shellism() {
        let tokens = tokenize("echo $(date)");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_dollar_brace_stays_shellism() {
        let tokens = tokenize("echo ${HOME}");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_dollar_special_vars_stay_shellism() {
        for s in &["echo $?", "echo $$", "echo $!"] {
            let tokens = tokenize(s);
            assert!(
                tokens.iter().any(|t| t.kind == TokenKind::Shellism),
                "{} should produce Shellism",
                s
            );
        }
    }

    #[test]
    fn test_dollar_digit_stays_shellism() {
        let tokens = tokenize("echo $1");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_quoted_variable_not_shellism() {
        let tokens = tokenize("echo \"$HOME\"");
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_backtick_substitution() {
        let tokens = tokenize("echo `date`");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_subshell_detection() {
        let tokens = tokenize("echo $(date)");
        let shellisms: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Shellism)
            .collect();
        assert!(!shellisms.is_empty());
    }

    #[test]
    fn test_brace_expansion() {
        let tokens = tokenize("echo {a,b}.txt");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_escaped_glob() {
        let tokens = tokenize("echo \\*.txt");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "*"));
    }

    #[test]
    fn test_redirect_out() {
        let tokens = tokenize("cmd > file");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_redirect_append() {
        let tokens = tokenize("cmd >> file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">>"));
    }

    #[test]
    fn test_redirect_in() {
        let tokens = tokenize("cmd < file");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_redirect_stderr() {
        let tokens = tokenize("cmd 2> file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value.starts_with("2>")));
    }

    #[test]
    fn test_redirect_stderr_no_space() {
        let tokens = tokenize("cmd 2>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "/dev/null"));
    }

    #[test]
    fn test_redirect_dev_null() {
        let tokens = tokenize("cmd > /dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">"));
    }

    #[test]
    fn test_redirect_2_to_1_single_token() {
        let tokens = tokenize("cmd 2>&1");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[1].kind, TokenKind::Redirect);
        assert_eq!(tokens[1].value, "2>&1");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
    }

    #[test]
    fn test_redirect_1_to_2_single_token() {
        let tokens = tokenize("cmd 1>&2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "1>&2"));
    }

    #[test]
    fn test_redirect_fd_close() {
        let tokens = tokenize("cmd 2>&-");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&-"));
    }

    #[test]
    fn test_redirect_shorthand_dup() {
        let tokens = tokenize("cmd >&2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">&2"));
    }

    #[test]
    fn test_redirect_amp_gt() {
        let tokens = tokenize("cmd &>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "&>"));
    }

    #[test]
    fn test_redirect_amp_gt_gt() {
        let tokens = tokenize("cmd &>>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "&>>"));
    }

    #[test]
    fn test_combined_redirect_chain() {
        let tokens = tokenize("cmd > /dev/null 2>&1");
        let redirects: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Redirect)
            .collect();
        assert_eq!(redirects.len(), 2);
        assert_eq!(redirects[0].value, ">");
        assert_eq!(redirects[1].value, "2>&1");
    }

    #[test]
    fn test_redirect_append_to_file() {
        let tokens = tokenize("echo hello >> /tmp/output.txt");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">>"));
    }

    #[test]
    fn test_redirect_heredoc_marker() {
        let tokens = tokenize("cat <<EOF");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "<<"));
    }

    #[test]
    fn test_redirect_2_to_1_with_pipe() {
        let tokens = tokenize("cargo test 2>&1 | head");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&1"));
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Pipe(_))));
    }

    #[test]
    fn test_redirect_2_to_1_with_and() {
        let tokens = tokenize("cargo test 2>&1 && echo done");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&1"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "&&"));
    }

    #[test]
    fn test_exclamation_is_shellism() {
        let tokens = tokenize("if ! grep -q pattern file; then echo missing; fi");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "!"));
    }

    #[test]
    fn test_background_job_is_shellism() {
        let tokens = tokenize("sleep 10 &");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
    }

    #[test]
    fn test_background_not_confused_with_amp_redirect() {
        let tokens = tokenize("cargo test &>/dev/null");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_semicolon_no_space() {
        let tokens = tokenize("git status;cargo test");
        assert_eq!(
            tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Operator)
                .count(),
            1
        );
        assert_eq!(
            tokens.iter().filter(|t| t.kind == TokenKind::Arg).count(),
            4
        );
    }

    #[test]
    fn test_offset_tracking() {
        let tokens = tokenize("a && b");
        assert_eq!(tokens[0].offset, 0);
        assert_eq!(tokens[1].offset, 2);
        assert_eq!(tokens[2].offset, 5);
    }

    #[test]
    fn test_offset_segment_extraction() {
        let cmd = "git add . && cargo test";
        let tokens = tokenize(cmd);
        let op = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Operator)
            .unwrap();
        let left = cmd[..op.offset].trim();
        let right_start = op.offset + op.value.len();
        let right = cmd[right_start..].trim();
        assert_eq!(left, "git add .");
        assert_eq!(right, "cargo test");
    }

    #[test]
    fn test_env_prefix_is_arg() {
        let tokens = tokenize("GIT_SSH_COMMAND=ssh git push");
        assert_eq!(tokens[0].kind, TokenKind::Arg);
        assert_eq!(tokens[0].value, "GIT_SSH_COMMAND=ssh");
    }

    #[test]
    fn test_complex_compound() {
        let tokens = tokenize("cargo fmt --all && cargo clippy --all-targets && cargo test");
        let operators: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(operators.len(), 2);
        assert!(operators.iter().all(|t| t.value == "&&"));
    }

    #[test]
    fn test_find_pipe_xargs() {
        let tokens = tokenize("find . -name '*.rs' | xargs grep 'fn run'");
        let pipe_idx = tokens
            .iter()
            .position(|t| matches!(t.kind, TokenKind::Pipe(_)))
            .unwrap();
        assert!(pipe_idx > 0);
        let before_pipe: Vec<_> = tokens[..pipe_idx]
            .iter()
            .filter(|t| t.kind == TokenKind::Arg)
            .collect();
        assert!(before_pipe.iter().any(|t| t.value == "find"));
    }

    #[test]
    fn test_fd_redirect_needs_adjacent_digit() {
        let tokens = tokenize("echo 2 > file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "2"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">"));
    }

    #[test]
    fn test_fd_redirect_no_space() {
        let tokens = tokenize("echo 2>file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "file"));
    }

    #[test]
    fn test_shell_split_simple() {
        assert_eq!(
            shell_split("head -50 file.php"),
            vec!["head", "-50", "file.php"]
        );
    }

    #[test]
    fn test_shell_split_double_quotes() {
        assert_eq!(
            shell_split(r#"git log --format="%H %s""#),
            vec!["git", "log", "--format=%H %s"]
        );
    }

    #[test]
    fn test_shell_split_single_quotes() {
        assert_eq!(
            shell_split("grep -r 'hello world' ."),
            vec!["grep", "-r", "hello world", "."]
        );
    }

    #[test]
    fn test_shell_split_single_word() {
        assert_eq!(shell_split("ls"), vec!["ls"]);
    }

    #[test]
    fn test_shell_split_empty() {
        let result: Vec<String> = shell_split("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_shell_split_backslash_escape() {
        assert_eq!(
            shell_split(r"echo hello\ world"),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_shell_split_unclosed_quote() {
        let result = shell_split("echo 'hello");
        assert_eq!(result, vec!["echo", "hello"]);
    }

    #[test]
    fn test_shell_split_mixed_quotes() {
        assert_eq!(
            shell_split(r#"echo "it's" 'a "test"'"#),
            vec!["echo", "it's", "a \"test\""]
        );
    }

    #[test]
    fn test_shell_split_tabs() {
        assert_eq!(shell_split("a\tb\tc"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_shell_split_multiple_spaces() {
        assert_eq!(shell_split("a   b   c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_strip_quotes_double() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
    }

    #[test]
    fn test_strip_quotes_single() {
        assert_eq!(strip_quotes("'hello'"), "hello");
    }

    #[test]
    fn test_strip_quotes_none() {
        assert_eq!(strip_quotes("hello"), "hello");
    }

    #[test]
    fn test_strip_quotes_mismatched() {
        assert_eq!(strip_quotes("\"hello'"), "\"hello'");
    }

    #[test]
    fn test_split_on_operators_stop_at_pipe() {
        assert_eq!(split_on_operators("a | b | c", true), vec!["a"]);
        assert_eq!(split_on_operators("a && b | c", true), vec!["a", "b"]);
    }

    #[test]
    fn test_split_on_operators_through_pipes() {
        assert_eq!(split_on_operators("a | b | c", false), vec!["a", "b", "c"]);
        assert_eq!(
            split_on_operators("a && b | c ; d", false),
            vec!["a", "b", "c", "d"]
        );
    }

    #[test]
    fn test_split_on_operators_quoted() {
        assert_eq!(
            split_on_operators(r#"echo "a && b" && cargo test"#, false),
            vec![r#"echo "a && b""#, "cargo test"]
        );
    }

    #[test]
    fn test_split_on_operators_empty() {
        assert!(split_on_operators("", false).is_empty());
        assert!(split_on_operators("  ", true).is_empty());
    }

    // --- contains_unattestable_construct (security) -------------------------

    #[test]
    fn test_unattestable_backtick() {
        assert!(contains_unattestable_construct("git status `whoami`"));
    }

    #[test]
    fn test_unattestable_command_substitution() {
        assert!(contains_unattestable_construct(
            "git log --pretty=$(rm -rf ~)"
        ));
    }

    #[test]
    fn test_unattestable_process_substitution() {
        assert!(contains_unattestable_construct("diff <(secret) <(other)"));
        assert!(contains_unattestable_construct("tee >(cat)"));
    }

    #[test]
    fn test_unattestable_substitution_inside_double_quotes() {
        assert!(contains_unattestable_construct(
            r#"git log --pretty="$(rm -rf ~)""#
        ));
        assert!(contains_unattestable_construct(
            r#"git log --pretty="`rm -rf ~`""#
        ));
        assert!(contains_unattestable_construct(
            r#"git -c x="$(whoami)" status"#
        ));
    }

    #[test]
    fn test_attestable_substitution_inside_single_quotes() {
        assert!(!contains_unattestable_construct("echo '$(rm -rf ~)'"));
        assert!(!contains_unattestable_construct("echo '`whoami`'"));
        assert!(!contains_unattestable_construct(r#"echo "\$(rm -rf ~)""#));
    }

    #[test]
    fn test_unattestable_file_redirects() {
        assert!(contains_unattestable_construct("git log > /tmp/x"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("echo evil >> ~/.bashrc"));
        assert!(contains_unattestable_construct("cmd &> /tmp/x"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("cat < /etc/passwd"));
        assert!(contains_unattestable_construct("cat << EOF"));
    }

    #[test]
    fn test_unattestable_ampersand_file_redirect() {
        // `>&word` (word not a number) == `>word 2>&1` — a file write.
        assert!(contains_unattestable_construct("git status >& /tmp/evil"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("cat x >&~/.bashrc"));
        assert!(contains_unattestable_construct("echo hi 2>& /tmp/evil"));
    }

    #[test]
    fn test_attestable_fd_dup_and_devnull_redirects() {
        assert!(!contains_unattestable_construct("git status 2>&1"));
        assert!(!contains_unattestable_construct("cmd >&2"));
        assert!(!contains_unattestable_construct("cmd 2>&-"));
        assert!(!contains_unattestable_construct("cmd 2>/dev/null"));
        assert!(!contains_unattestable_construct("cmd > /dev/null"));
        assert!(!contains_unattestable_construct("cmd &> /dev/null"));
        assert!(!contains_unattestable_construct("cmd >& /dev/null"));
    }

    #[test]
    fn test_stdout_file_redirects() {
        assert!(has_stdout_file_redirect("rg needle . > matches.txt"));
        assert!(has_stdout_file_redirect("rg needle . 1>>matches.txt"));
        assert!(has_stdout_file_redirect("rg needle . &> matches.txt"));
        assert!(has_stdout_file_redirect(
            "rg needle . | awk '{print $1}' > matches.txt"
        ));
    }

    #[test]
    fn test_non_file_or_non_stdout_redirects() {
        assert!(!has_stdout_file_redirect("rg needle . 2> errors.txt"));
        assert!(!has_stdout_file_redirect("rg needle . 2>&1"));
        assert!(!has_stdout_file_redirect("rg needle . > /dev/null"));
        assert!(!has_stdout_file_redirect("cat < input.txt"));
    }

    #[test]
    fn test_attestable_subshell_and_separators() {
        assert!(!contains_unattestable_construct(
            "(git status; cargo build)"
        ));
        assert!(!contains_unattestable_construct(
            "git status && cargo build"
        ));
        assert!(!contains_unattestable_construct("git status; cargo build"));
        assert!(!contains_unattestable_construct("git log | head"));
        assert!(!contains_unattestable_construct("sleep 1 &"));
        assert!(!contains_unattestable_construct("git status\ncargo build"));
    }

    #[test]
    fn test_attestable_variable_expansion() {
        assert!(!contains_unattestable_construct("echo $HOME"));
        assert!(!contains_unattestable_construct("echo ${HOME}"));
        assert!(!contains_unattestable_construct("git status"));
        assert!(!contains_unattestable_construct(""));
    }

    #[test]
    fn test_shell_wrapper_payload_preserves_outer_invocation() {
        let invocation = "/bin/sh -c 'git status --short' positional";
        let ShellWrapperParse::Parsed(payload) = parse_shell_command_wrapper(invocation) else {
            panic!("expected parsed shell wrapper");
        };
        assert_eq!(payload.command, "git status --short");
        assert_eq!(
            payload.replace_in(invocation, "rtk git status --short"),
            "/bin/sh -c 'rtk git status --short' positional"
        );
    }

    #[test]
    fn test_shell_wrapper_parser_supports_login_command_flags_and_nesting() {
        let ShellWrapperParse::Parsed(payload) =
            parse_shell_command_wrapper("bash -lc 'sh -c \"bun run check\"'")
        else {
            panic!("expected parsed shell wrapper");
        };
        assert_eq!(payload.command, "sh -c \"bun run check\"");
        let ShellWrapperParse::Parsed(nested) = parse_shell_command_wrapper(&payload.command)
        else {
            panic!("expected nested shell wrapper");
        };
        assert_eq!(nested.command, "bun run check");
        assert!(matches!(
            parse_shell_command_wrapper("bash -O extglob -lc 'git status'"),
            ShellWrapperParse::Parsed(_)
        ));
    }

    #[test]
    fn test_shell_wrapper_parser_rejects_malformed_or_opaque_syntax() {
        assert_eq!(
            parse_shell_command_wrapper("sh -c 'git status"),
            ShellWrapperParse::Ambiguous
        );
        assert_eq!(
            parse_shell_command_wrapper("sh -c 'git status' | cat"),
            ShellWrapperParse::Ambiguous
        );
        assert_eq!(
            parse_shell_command_wrapper("python -c 'print(1)'"),
            ShellWrapperParse::NotWrapper
        );
        assert_eq!(
            parse_shell_command_wrapper("sh script.sh -c 'git status'"),
            ShellWrapperParse::Ambiguous
        );
    }

    // --- split_for_permissions ---------------------------------------------

    #[test]
    fn test_split_perms_operators() {
        assert_eq!(
            split_for_permissions("git status && cargo build"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(
            split_for_permissions("git status; cargo build"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(
            split_for_permissions("git log | head"),
            vec!["git log", "head"]
        );
    }

    #[test]
    fn test_split_perms_newline() {
        assert_eq!(
            split_for_permissions("git status\ncargo build"),
            vec!["git status", "cargo build"]
        );
    }

    #[test]
    fn test_split_perms_background_ampersand() {
        assert_eq!(
            split_for_permissions("git status & rm -rf ~"),
            vec!["git status", "rm -rf ~"]
        );
        assert_eq!(split_for_permissions("sleep 1 &"), vec!["sleep 1"]);
    }

    #[test]
    fn test_split_perms_subshell() {
        assert_eq!(
            split_for_permissions("(git status; cargo build)"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(split_for_permissions("((a; b); c)"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_split_perms_truncates_at_redirect() {
        assert_eq!(split_for_permissions("git status 2>&1"), vec!["git status"]);
        assert_eq!(split_for_permissions("git log > /tmp/x"), vec!["git log"]);
        assert_eq!(
            split_for_permissions("git push --force 2>&1"),
            vec!["git push --force"]
        );
    }

    #[test]
    fn test_split_perms_newline_inside_quotes_not_split() {
        let segments = split_for_permissions("echo 'line1\nline2'");
        assert_eq!(segments.len(), 1);
        assert!(segments[0].starts_with("echo"));
    }

    #[test]
    fn test_split_perms_empty() {
        assert!(split_for_permissions("").is_empty());
        assert!(split_for_permissions("   ").is_empty());
    }

    #[test]
    fn test_tokenize_with_newlines_emits_operator_outside_quotes_only() {
        let newline_ops = |input: &str| {
            tokenize_with_newlines(input)
                .iter()
                .filter(|t| t.kind == TokenKind::Operator && t.value == "\n")
                .count()
        };
        assert_eq!(newline_ops("git status\ngit log"), 1);
        assert_eq!(newline_ops("echo 'line1\nline2'"), 0);
        assert_eq!(newline_ops("git status\r\ngit log"), 2);
    }
}
