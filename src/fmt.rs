//! `kaze fmt` — the canonical source formatter.
//!
//! The formatter is deliberately **conservative**. It validates the source with
//! the parser, then normalizes:
//!
//!   * indentation to four spaces per block level (continuation lines inside
//!     `(`/`[` get one extra level),
//!   * spacing between tokens (`a=1+2` -> `a = 1 + 2`, `f (a , b)` -> `f(a, b)`,
//!     `p . x` -> `p.x`, `1m/s` -> `1 m/s`),
//!   * CRLF line endings and trailing whitespace. Blank lines are preserved
//!     (trailing ones trimmed): physical line numbers are observable through
//!     `err.line`, so collapsing a run would change behaviour.
//!
//! It never merges or splits physical code lines, and it preserves every
//! comment. Before returning, it tokenizes both the input and the output and
//! refuses if the token streams differ: a formatter that changes what a script
//! means is a bug, not a convenience.

use crate::arena::fmt_num;
use crate::lexer::{tokenize, Tok};
use crate::KazeError;

/// Format one source string. `name` is used in parse-error messages.
pub fn format_source(src: &str, name: &str) -> Result<String, KazeError> {
    // Parse first: a formatter that rewrites broken code is worse than one
    // that says where the syntax error is. Semantic checks are not run, so a
    // script can be formatted while its names are still in flux.
    crate::parser::parse(src)
        .map_err(|e| KazeError::new("E_KAZE_PARSE", e.render(name)))?;

    // Normalize real line endings only. A raw CR inside a string (or comment)
    // is part of that token; replacing it would split a line the parser
    // accepted.
    let normalized = src.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();

    // Bracket depth at the start of each physical line, so continuation lines
    // can be indented and excluded from block-level discovery.
    let mut start_depth = Vec::with_capacity(lines.len());
    let mut depth: i32 = 0;
    for line in &lines {
        start_depth.push(depth);
        let (code, _) = split_comment(line);
        if code.trim().is_empty() {
            continue;
        }
        if let Ok(tokens) = tokenize(code.trim_start()) {
            for t in tokens {
                match t.tok {
                    Tok::Punct('(') | Tok::Punct('[') => depth += 1,
                    Tok::Punct(')') | Tok::Punct(']') => depth = (depth - 1).max(0),
                    _ => {}
                }
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    // Stack of source indentation widths. It starts with 0 and the pop loop
    // keeps at least one entry, so `levels.last()` is always Some.
    let mut levels: Vec<usize> = vec![0];
    let mut block_depth: usize = 0;

    for (index, raw) in lines.iter().enumerate() {
        let (code, comment) = split_comment(raw);
        let code = code.trim_end();
        let leading = raw.chars().take_while(|c| *c == ' ').count();
        let bracket = start_depth[index].max(0) as usize;
        let continuation = bracket > 0;

        if code.trim().is_empty() && comment.is_none() {
            // Preserve every blank line (trailing ones are trimmed below).
            // Physical line numbers are observable through `err.line` in a
            // recovery block, so collapsing a run would change behaviour.
            out.push(String::new());
            continue;
        }

        if code.trim().is_empty() {
            // Comment-only line: align with its source column, mapped to the
            // nearest canonical level. A comment one level deeper than the
            // current block (a comment just under a header) keeps that depth.
            let depth = if continuation {
                block_depth + bracket
            } else if leading > *levels.last().unwrap() {
                levels.len()
            } else {
                levels.iter().rposition(|level| *level <= leading).unwrap_or(0)
            };
            let text = comment.unwrap_or("").trim_end();
            out.push(format!("{}{}", "    ".repeat(depth), text));
            continue;
        }

        if !continuation {
            while levels.len() > 1 && leading < *levels.last().unwrap() {
                levels.pop();
            }
            if leading > *levels.last().unwrap() {
                levels.push(leading);
            }
            block_depth = levels.len() - 1;
        }

        let tokens: Vec<Tok> = tokenize(code.trim_start())
            .map_err(|e| KazeError::new("E_KAZE_PARSE", e.render(name)))?
            .into_iter()
            .map(|t| t.tok)
            .filter(|t| !matches!(t, Tok::Newline | Tok::Indent | Tok::Dedent | Tok::Eof))
            .collect();
        let rendered = render_tokens(&tokens);
        // Continuation lines indent by their real bracket depth, so nested
        // lists keep their relative shape. A closing bracket aligns with the
        // line that opened it (one level less than the bracket depth it closes).
        let closing = matches!(
            tokens.first(),
            Some(Tok::Punct(']') | Tok::Punct(')'))
        );
        let depth = if continuation {
            block_depth + bracket.saturating_sub(usize::from(closing))
        } else {
            block_depth
        };
        let mut line = format!("{}{}", "    ".repeat(depth), rendered);
        if let Some(comment) = comment {
            let comment = comment.trim_end();
            if !comment.is_empty() {
                line.push_str("  ");
                line.push_str(comment);
            }
        }
        out.push(line);
    }

    while out.last().is_some_and(|line| line.is_empty()) {
        out.pop();
    }
    let formatted = if out.is_empty() { String::new() } else { format!("{}\n", out.join("\n")) };

    // Safety net: the whitespace-normalized token streams must be identical.
    if token_stream(src) != token_stream(&formatted) {
        return Err(KazeError::new(
            "E_KAZE_FMT",
            format!(
                "{}: refusing to format: the result would change this script's tokens (formatter bug)",
                name
            ),
        ));
    }
    Ok(formatted)
}

fn token_stream(src: &str) -> Option<Vec<Tok>> {
    tokenize(src)
        .ok()
        .map(|tokens| tokens.into_iter().map(|t| t.tok).collect())
}

/// Split a physical line at the first `#` that is not inside a string.
fn split_comment(line: &str) -> (&str, Option<&str>) {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == b'#' {
            return (&line[..i], Some(&line[i..]));
        }
        i += 1;
    }
    (line, None)
}

fn render_tokens(tokens: &[Tok]) -> String {
    let mut out = String::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i > 0 && space_before(tokens, i) {
            out.push(' ');
        }
        match tok {
            Tok::Ident(s) => out.push_str(s),
            Tok::Num(n) => out.push_str(&fmt_num(*n)),
            Tok::Str(s) => {
                out.push('"');
                for c in s.chars() {
                    match c {
                        '\\' => out.push_str("\\\\"),
                        '"' => out.push_str("\\\""),
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        other => out.push(other),
                    }
                }
                out.push('"');
            }
            Tok::Punct(c) => out.push(*c),
            Tok::Le => out.push_str("<="),
            Tok::Ge => out.push_str(">="),
            Tok::EqEq => out.push_str("=="),
            Tok::Ne => out.push_str("!="),
            Tok::Newline | Tok::Indent | Tok::Dedent | Tok::Eof => {}
        }
    }
    out
}

fn space_before(tokens: &[Tok], i: usize) -> bool {
    let cur = &tokens[i];
    let prev = &tokens[i - 1];
    let prev2 = i.checked_sub(2).map(|j| &tokens[j]);
    let next = tokens.get(i + 1);
    let prev3 = i.checked_sub(3).map(|j| &tokens[j]);

    // No space after an opening bracket, a dot, or a unary sign.
    if matches!(prev, Tok::Punct('(') | Tok::Punct('[') | Tok::Punct('.')) {
        return false;
    }
    if matches!(prev, Tok::Punct('-') | Tok::Punct('+')) && unary_at(tokens, i - 1) {
        return false;
    }
    // `m/s` is a unit, not a division: write it closed up.
    if let Tok::Punct('/') = cur {
        if matches!(prev, Tok::Ident(u) if is_meter(u))
            && matches!(prev2, Some(Tok::Num(_)))
            && matches!(next, Some(Tok::Ident(s)) if s == "s")
        {
            return false;
        }
    }
    if matches!(cur, Tok::Ident(s) if s == "s")
        && matches!(prev, Tok::Punct('/'))
        && matches!(prev2, Some(Tok::Ident(u)) if is_meter(u))
        && matches!(prev3, Some(Tok::Num(_)))
    {
        return false;
    }

    match cur {
        // Closing punctuation and separators hug the left token.
        Tok::Punct(',') | Tok::Punct(':') | Tok::Punct(')') | Tok::Punct(']') | Tok::Punct('.') => false,
        // Calls and item headers hug `(`; grouping and keywords get a space.
        Tok::Punct('(') => !matches!(
            prev,
            Tok::Ident(name)
                if matches!(prev2, Some(Tok::Punct('.')))
                    || matches!(
                        name.as_str(),
                        "vector" | "sin" | "cos" | "atan2" | "sqrt"
                    )
                    || matches!(
                        prev2,
                        Some(Tok::Ident(kw))
                            if matches!(kw.as_str(), "action" | "check" | "filter")
                    )
        ),
        // Indexing hugs the value; a list literal follows an operator/`=` or a
        // keyword (`for each p in [ … ]`). `in[` is valid but wrong-looking.
        Tok::Punct('[') => match prev {
            Tok::Ident(name) if !is_keyword_name(name) => false,
            Tok::Num(_) | Tok::Str(_) | Tok::Punct(')') | Tok::Punct(']') => false,
            _ => true,
        },
        // `20%` is a unit; `a % b` is modulo.
        Tok::Punct('%') => !matches!(prev, Tok::Num(_)),
        // A sign hugs `(`/`[`; everywhere else it follows a space, including
        // after `=`, a comma and another operator (`x = -5`, `f(a, -1)`).
        // The operand itself hugs the sign (handled above).
        Tok::Punct('-') | Tok::Punct('+') => {
            !matches!(prev, Tok::Punct('(') | Tok::Punct('['))
        }
        Tok::Punct('*') | Tok::Punct('/') | Tok::Punct('<') | Tok::Punct('>') | Tok::Punct('=') => true,
        Tok::Le | Tok::Ge | Tok::EqEq | Tok::Ne => true,
        _ => true,
    }
}

fn unary_at(tokens: &[Tok], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    match &tokens[i - 1] {
        Tok::Punct('(')
        | Tok::Punct('[')
        | Tok::Punct(',')
        | Tok::Punct(':')
        | Tok::Punct('=')
        | Tok::Punct('+')
        | Tok::Punct('-')
        | Tok::Punct('*')
        | Tok::Punct('/')
        | Tok::Punct('%')
        | Tok::Punct('<')
        | Tok::Punct('>')
        | Tok::Le
        | Tok::Ge
        | Tok::EqEq
        | Tok::Ne => true,
        Tok::Ident(w) => {
            matches!(w.as_str(), "and" | "or" | "not" | "return") || is_keyword_name(w)
        }
        _ => false,
    }
}

fn is_meter(word: &str) -> bool {
    matches!(word, "m" | "meter" | "meters")
}

/// Words that are never values, so a following `[` is a list literal rather
/// than an index. Kept deliberately small: a bound variable that happens to be
/// named `vector` still indexes.
fn is_keyword_name(word: &str) -> bool {
    matches!(
        word,
        "in"
            | "of"
            | "and"
            | "or"
            | "not"
            | "return"
            | "to"
            | "at"
            | "with"
            | "as"
            | "for"
            | "each"
            | "let"
            | "set"
            | "if"
            | "otherwise"
            | "while"
            | "when"
            | "every"
            | "action"
            | "check"
            | "filter"
            | "try"
            | "needs"
            | "use"
            | "log"
            | "send"
            | "alert"
            | "remember"
            | "allow"
            | "reject"
            | "pulse"
            | "turn"
            | "on"
            | "off"
            | "arm"
            | "takeoff"
            | "fly"
            | "land"
            | "hold"
            | "stop"
            | "drive"
            | "forward"
            | "backward"
            | "recall"
            | "memories"
            | "matching"
            | "length"
            | "distance"
            | "between"
            | "slant"
            | "bearing"
            | "from"
            | "vector"
            | "sin"
            | "cos"
            | "atan2"
            | "sqrt"
            | "radians"
            | "degrees"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format_source(src, "<fmt>").expect("formats")
    }

    #[test]
    fn spacing_is_canonical() {
        let src = "needs:log\naction main():\n  let x=1+2\n  log\"hi\"\n";
        assert_eq!(
            fmt(src),
            "needs: log\naction main():\n    let x = 1 + 2\n    log \"hi\"\n"
        );
    }

    #[test]
    fn calls_points_and_units() {
        let src = "needs: drone\naction main():\n    let p = vector( 1,2 , 3 )\n    log p . x\n    drone . takeoff ( 1 m / s )\n";
        assert_eq!(
            fmt(src),
            "needs: drone\naction main():\n    let p = vector(1, 2, 3)\n    log p.x\n    drone.takeoff(1 m/s)\n"
        );
    }

    #[test]
    fn comments_survive_and_align() {
        let src = "# top\naction main():   # header\n  # body note\n  log \"x\"  # trail\n";
        assert_eq!(
            fmt(src),
            "# top\naction main():  # header\n    # body note\n    log \"x\"  # trail\n"
        );
    }

    #[test]
    fn multi_line_lists_keep_a_space_after_in_and_align_the_bracket() {
        let src = "needs: log\naction main():\n    for each p in[\n        vector(1, 2, 3),\n        vector(4, 5, 6)\n        ]:\n        log p\n";
        assert_eq!(
            fmt(src),
            "needs: log\naction main():\n    for each p in [\n        vector(1, 2, 3),\n        vector(4, 5, 6)\n    ]:\n        log p\n"
        );
    }

    #[test]
    fn idempotent() {
        let src = "needs: log\naction main():\n    if 1==1:\n        log \"a\"\n    otherwise:\n        log \"b\"\n";
        let once = fmt(src);
        assert_eq!(fmt(&once), once);
    }

    #[test]
    fn rejects_broken_source() {
        let err = format_source("action main(:\n", "<fmt>").unwrap_err();
        assert_eq!(err.code, "E_KAZE_PARSE");
    }
}
