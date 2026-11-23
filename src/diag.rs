// sfs/diag.rs -- rendering a diagnostic the way a person reads one.
//
// Phase 2 of the roadmap asks for "the exact file path, line, column, multiline
// context snippet, visual caret indicators, and concrete suggestions". The
// checker knows *what* is wrong and the parser knows line+col; this module turns
// that into:
//
//     error[E_KAZE_UNIT_MISMATCH]: cannot add length (meter) and time (second)
//       --> flight/control.kaze:22:15
//        |
//     22 |   let target = altitude + current_speed
//        |                      ^ here: length (meter) and time (second)
//
// The column is located by searching the offending line for a needle (the
// operator, the identifier, the bracket). That keeps the AST free of column
// tracking while still pointing at the right character; when the needle is not
// on the line the caret falls back to column 1 rather than lying about a
// position it does not know.

/// A line index over one source text.
pub struct SourceMap<'a> {
    text: &'a str,
    /// Byte offset where each line starts. starts[0] is 0.
    starts: Vec<usize>,
}

impl<'a> SourceMap<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        SourceMap { text, starts }
    }

    /// The text of a 1-based line, without its newline.
    pub fn line(&self, line: u32) -> &'a str {
        if line == 0 {
            return "";
        }
        let idx = (line - 1) as usize;
        let Some(&start) = self.starts.get(idx) else {
            return "";
        };
        let end = self
            .starts
            .get(idx + 1)
            .map(|n| n.saturating_sub(1))
            .unwrap_or(self.text.len());
        self.text[start.min(self.text.len())..end.min(self.text.len())].trim_end_matches('\r')
    }

    /// Locate a needle on a 1-based line: the 1-based column and caret width.
    pub fn locate(&self, line: u32, needles: &[&str]) -> Option<(u32, u32)> {
        let text = self.line(line);
        if text.is_empty() {
            return None;
        }
        for needle in needles {
            if needle.is_empty() {
                continue;
            }
            if let Some(byte_at) = text.find(needle) {
                let col = text[..byte_at].chars().count() as u32 + 1;
                let width = needle.chars().count().max(1) as u32;
                return Some((col, width));
            }
        }
        None
    }
}


/// One labelled span inside a diagnostic: the text to find on the line and the
/// label to print under it.
pub struct Span {
    pub needle: String,
    pub label: String,
}

impl SourceMap<'_> {
    /// Locate several needles on one line, left to right, skipping earlier
    /// occurrences so two spans with the same text stay distinct.
    pub fn locate_all(&self, line: u32, needles: &[&str]) -> Vec<(u32, u32)> {
        let text = self.line(line);
        let mut out = Vec::new();
        if text.is_empty() {
            return out;
        }
        let mut searched = 0usize;
        for needle in needles {
            if needle.is_empty() {
                continue;
            }
            let mut from = searched;
            let mut found = None;
            while let Some(at) = text[from..].find(needle) {
                let at = from + at;
                // A word must not be found inside a longer word: `meter` should
                // not match inside `meters`.
                let before_ok = at == 0 || !is_word(text.as_bytes()[at - 1]);
                let after = at + needle.len();
                let after_ok = after >= text.len() || !is_word(text.as_bytes()[after]);
                if before_ok && after_ok {
                    found = Some(at);
                    break;
                }
                from = at + 1;
            }
            if let Some(at) = found {
                let col = text[..at].chars().count() as u32 + 1;
                let width = needle.chars().count().max(1) as u32;
                out.push((col, width));
                searched = at + needle.len();
            }
        }
        out
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Render a diagnostic with several labelled spans, the way the roadmap draws a
/// unit mismatch:
///
/// ```text
/// error[E_KAZE_UNIT_MISMATCH]: cannot add length (meter) and time (second)
///       --> line 3, column 21
///        |
///      3 |     let total = runway_length + flight_time
///        |                   ^^^^^^^^^^^^^   ^^^^^^^^^^^
///        |                   |               |
///        |                   |               here: time (second)
///        |                   here: length (meter)
/// ```
pub fn render_spans(
    code: &str,
    message: &str,
    file: &str,
    line: u32,
    spans: &[Span],
    source: Option<&SourceMap<'_>>,
) -> Option<String> {
    if spans.is_empty() {
        return None;
    }
    let map = source?;
    let line = line.max(1);
    let text = map.line(line);
    if text.is_empty() {
        return None;
    }
    let needles: Vec<&str> = spans.iter().map(|s| s.needle.as_str()).collect();
    let located = map.locate_all(line, &needles);
    if located.len() != spans.len() {
        return None;
    }
    // One caret per span, ordered left to right; keep each label with its span.
    let mut marks: Vec<(u32, u32, &str)> = located
        .iter()
        .zip(spans)
        .map(|((col, width), span)| (*col, *width, span.label.as_str()))
        .collect();
    marks.sort_by_key(|(col, _, _)| *col);

    // A message may end with its own code; the header already carries it.
    let mut text_msg = message.trim_end().to_string();
    if let Some((head, tail)) = text_msg.rsplit_once(": ") {
        if tail == code {
            text_msg = head.to_string();
        }
    }
    let message = text_msg.as_str();
    let gutter = format!("{}", line).len().max(2);
    let pad = " ".repeat(gutter);
    let mut out = String::new();
    out.push_str(&format!("error[{}]: {}\n", code, message));
    out.push_str(&format!("  --> {}:{}:{}\n", file, line, marks[0].0));
    out.push_str(&format!("{} |\n", pad));
    out.push_str(&format!("{} | {}\n", line, text));

    // The caret row: a run of carets under each span, spaces elsewhere.
    let mut carets = String::new();
    let mut at = 1u32;
    for (col, width, _) in &marks {
        while at < *col {
            carets.push(' ');
            at += 1;
        }
        for _ in 0..(*width).max(1) {
            carets.push('^');
            at += 1;
        }
    }
    out.push_str(&format!("{} | {}\n", pad, carets));

    // Label rows: a connector column per span, then one label per row from the
    // rightmost span to the leftmost, matching the caret positions.
    if marks.len() == 1 {
        out.push_str(&format!("{} | {} {}\n", pad, carets, marks[0].2));
        return Some(out);
    }
    let columns: Vec<u32> = marks.iter().map(|(col, _, _)| *col).collect();
    // connector line
    let mut conn = String::new();
    let mut at = 1u32;
    for col in &columns {
        while at < *col {
            conn.push(' ');
            at += 1;
        }
        conn.push('|');
        at += 1;
    }
    out.push_str(&format!("{} | {}\n", pad, conn));
    // Each label row carries the connectors for the spans at or before its own
    // column, then the label -- the shape the roadmap draws.
    for (i, (_, _, label)) in marks.iter().enumerate().rev() {
        let col = columns[i];
        let mut row = String::new();
        let mut at = 1u32;
        for c in columns.iter() {
            if *c > col {
                break;
            }
            while at < *c {
                row.push(' ');
                at += 1;
            }
            row.push('|');
            at += 1;
        }
        row.push_str(&format!(" here: {}", label));
        out.push_str(&format!("{} | {}\n", pad, row));
    }
    Some(out)
}

/// Render a diagnostic block: header, location, snippet, caret, suggestion.
pub fn render(
    code: &str,
    message: &str,
    file: &str,
    line: u32,
    caret: Option<(u32, u32)>,
    hint: Option<&str>,
    source: Option<&SourceMap<'_>>,
) -> String {
    let (col, width) = caret.unwrap_or((1, 1));
    // A message may end with its own code ("... otherwise <fallback>: E_X").
    // The header already carries the code, so drop the echo when it matches.
    let mut text_msg = message.trim_end().to_string();
    if let Some((head, tail)) = text_msg.rsplit_once(": ") {
        if tail == code {
            text_msg = head.to_string();
        }
    }
    let message = text_msg.as_str();
    let mut out = String::new();
    out.push_str(&format!("error[{}]: {}\n", code, message));
    out.push_str(&format!("  --> {}:{}:{}\n", file, line.max(1), col));
    let Some(map) = source else {
        return out;
    };
    // A diagnostic can point one line past the last line (an unexpected end of
    // input). Show the last real line and put the caret at its end instead of
    // printing an empty snippet.
    let mut line = line.max(1);
    let mut text = map.line(line);
    if text.is_empty() && line > 1 {
        line -= 1;
        text = map.line(line);
        if !text.trim().is_empty() {
            let col = text.chars().count() as u32 + 1;
            let mut out = String::new();
            out.push_str(&format!("error[{}]: {}\n", code, message));
            out.push_str(&format!("  --> {}:{}:{}\n", file, line, col));
            let gutter = format!("{}", line).len().max(2);
            let pad = " ".repeat(gutter);
            out.push_str(&format!("{} |\n", pad));
            out.push_str(&format!("{} | {}\n", line, text));
            out.push_str(&format!("{} | {}^\n", pad, " ".repeat(col.saturating_sub(1) as usize)));
            if let Some(hint) = hint {
                out.push_str(&format!("{} | {}^ {}\n", pad, " ".repeat(col.saturating_sub(1) as usize), hint));
            }
            return out;
        }
    }
    if text.is_empty() {
        return out;
    }
    let gutter = format!("{}", line.max(1)).len().max(2);
    let pad = " ".repeat(gutter);
    let indent = " ".repeat(col.saturating_sub(1) as usize);
    let carets = "^".repeat(width.max(1) as usize);
    out.push_str(&format!("{} |\n", pad));
    out.push_str(&format!("{} | {}\n", line.max(1), text));
    out.push_str(&format!("{} | {}{}\n", pad, indent, carets));
    if let Some(hint) = hint {
        out.push_str(&format!("{} | {}{} {}\n", pad, indent, "^", hint));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "needs: log\naction main():\n    let x = 1 meter + 2 seconds\n    return x\n";

    #[test]
    fn a_block_points_at_the_needle() {
        let map = SourceMap::new(SRC);
        let caret = map.locate(3, &["+"]);
        assert_eq!(caret, Some((21, 1)), "the plus is at column 21");
        let block = render(
            "E_KAZE_UNIT_MISMATCH",
            "cannot add length and time",
            "x.kaze",
            3,
            caret,
            Some("here: length and time"),
            Some(&map),
        );
        let lines: Vec<&str> = block.lines().collect();
        assert!(lines[0].starts_with("error[E_KAZE_UNIT_MISMATCH]"), "{:?}", lines[0]);
        assert_eq!(lines[1], "  --> x.kaze:3:21");
        assert_eq!(
            lines[3],
            "3 |     let x = 1 meter + 2 seconds",
            "the snippet keeps its indentation",
        );
        let caret_line = lines[4];
        let col = caret_line.chars().position(|c| c == '^').expect("a caret");
        assert_eq!(
            col,
            "   | ".len() + 20,
            "the caret sits under the operator: {:?}",
            caret_line
        );
    }

    #[test]
    fn the_file_path_renders_a_block() {
        let err = crate::check_file("/tmp/d1.kaze").expect_err("a unit mismatch");
        eprintln!("--- library message ---\n{}", err.message);
        assert!(err.message.contains("    let x = 1 meter + 2 seconds"), "snippet keeps indentation: {}", err.message);
        assert!(err.message.contains("--> "), "has a location line");
    }

    #[test]
    fn two_spans_draw_two_carets_and_two_labels() {
        let src = "action main():\n    let runway_length = 1200 meter\n    let flight_time = 40 second\n    let total = runway_length + flight_time\n    return total\n";
        let map = SourceMap::new(src);
        let spans = [
            Span { needle: "runway_length".into(), label: "length (meter)".into() },
            Span { needle: "flight_time".into(), label: "time (second)".into() },
        ];
        let block = render_spans(
            "E_KAZE_UNIT_MISMATCH",
            "cannot add length (meter) and time (second)",
            "u.kaze",
            4,
            &spans,
            Some(&map),
        )
        .expect("renders");
        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(lines[0], "error[E_KAZE_UNIT_MISMATCH]: cannot add length (meter) and time (second)");
        assert_eq!(lines[1], "  --> u.kaze:4:17");
        // one caret run per operand
        let caret_row = lines.iter().find(|l| l.contains('^')).expect("carets");
        assert_eq!(caret_row.matches('^').count(), 13 + 11, "carets cover both operands: {:?}", caret_row);
        // both labels, each on its own row
        assert!(block.contains("here: time (second)"), "{}", block);
        assert!(block.contains("here: length (meter)"), "{}", block);
    }

    #[test]
    fn locate_all_keeps_repeated_needles_apart() {
        let map = SourceMap::new("let x = a + a\n");
        let cols = map.locate_all(1, &["a", "a"]);
        assert_eq!(cols.len(), 2);
        assert_ne!(cols[0].0, cols[1].0, "the second `a` is a different span");
    }
}
