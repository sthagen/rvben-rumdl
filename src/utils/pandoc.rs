//! Pandoc Markdown syntax detection.
//!
//! This module provides detection for Pandoc Markdown constructs that affect
//! rumdl rule output: fenced divs (`:::`), attribute lists (`{#id .class}`),
//! citations (`[@key]`), bracketed spans (`[text]{.class}`), and other
//! Pandoc-specific syntax.
//!
//! Pandoc is the foundation; the Quarto flavor extends it with Quarto-only
//! syntax (executable code blocks, shortcodes, cell options) elsewhere in
//! the codebase. Anything that's pure Pandoc lives here.
//!
//! Common patterns this module handles:
//! - `::: {.callout-note}` — fenced div with class
//! - `::: {#myid .class}` — generic div with id and class
//! - `:::` — closing marker
//! - `{#id .class key="value"}` — Pandoc attribute lists
//! - `@key`, `[@key]`, `[-@key]`, `[@a; @b]` — citations

use regex::Regex;
use std::sync::LazyLock;

use crate::utils::skip_context::ByteRange;

/// Pattern to match div opening markers
/// Matches: ::: {.class}, ::: {#id .class}, ::: classname, etc.
/// Does NOT match a closing ::: on its own
static DIV_OPEN_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*):::\s*(?:\{[^}]+\}|\S+)").unwrap());

/// Pattern to match div closing markers
/// Matches: ::: (with optional whitespace before and after)
static DIV_CLOSE_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*):::\s*$").unwrap());

/// Pattern to match callout blocks specifically
/// Callout types: note, warning, tip, important, caution
static CALLOUT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\s*):::\s*\{[^}]*\.callout-(?:note|warning|tip|important|caution)[^}]*\}").unwrap()
});

/// Pattern to match Pandoc-style attributes on any element
/// Matches: {#id}, {.class}, {#id .class key="value"}, etc.
/// Note: We match the entire attribute block including contents
static PANDOC_ATTR_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[^}]+\}").unwrap());

/// Check if a line is a div opening marker
pub fn is_div_open(line: &str) -> bool {
    DIV_OPEN_PATTERN.is_match(line)
}

/// Check if a line is a div closing marker (just `:::`)
pub fn is_div_close(line: &str) -> bool {
    DIV_CLOSE_PATTERN.is_match(line)
}

/// Check if a line is a callout block opening
pub fn is_callout_open(line: &str) -> bool {
    CALLOUT_PATTERN.is_match(line)
}

/// Check if a line contains Pandoc-style attributes
pub fn has_pandoc_attributes(line: &str) -> bool {
    PANDOC_ATTR_PATTERN.is_match(line)
}

/// Get the indentation level of a div marker
pub fn get_div_indent(line: &str) -> usize {
    let mut indent = 0;
    for c in line.chars() {
        match c {
            ' ' => indent += 1,
            '\t' => indent += 4, // Tabs expand to 4 spaces (CommonMark)
            _ => break,
        }
    }
    indent
}

/// Track div nesting state for a document
#[derive(Debug, Clone, Default)]
pub struct DivTracker {
    /// Stack of div indentation levels for nesting tracking
    indent_stack: Vec<usize>,
}

impl DivTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a line and return whether we're inside a div after processing
    pub fn process_line(&mut self, line: &str) -> bool {
        let trimmed = line.trim_start();

        if trimmed.starts_with(":::") {
            let indent = get_div_indent(line);

            if is_div_close(line) {
                // Closing marker - pop the matching div from stack
                // Pop the top div if its indent is >= the closing marker's indent
                if let Some(&top_indent) = self.indent_stack.last()
                    && top_indent >= indent
                {
                    self.indent_stack.pop();
                }
            } else if is_div_open(line) {
                // Opening marker - push to stack
                self.indent_stack.push(indent);
            }
        }

        !self.indent_stack.is_empty()
    }

    /// Check if we're currently inside a div
    pub fn is_inside_div(&self) -> bool {
        !self.indent_stack.is_empty()
    }
}

/// Detect fenced div block ranges in content.
/// Returns a vector of byte ranges (start, end) for each div block.
pub fn detect_div_block_ranges(content: &str) -> Vec<ByteRange> {
    let mut ranges = Vec::new();
    let mut tracker = DivTracker::new();
    let mut div_start: Option<usize> = None;
    let mut byte_offset = 0;

    for line in content.lines() {
        let line_len = line.len();
        let was_inside = tracker.is_inside_div();
        let is_inside = tracker.process_line(line);

        // Started a new div block
        if !was_inside && is_inside {
            div_start = Some(byte_offset);
        }
        // Exited a div block
        else if was_inside
            && !is_inside
            && let Some(start) = div_start.take()
        {
            // End at the start of the closing line
            ranges.push(ByteRange {
                start,
                end: byte_offset + line_len,
            });
        }

        // Account for newline
        byte_offset += line_len + 1;
    }

    // Handle unclosed divs at end of document
    if let Some(start) = div_start {
        ranges.push(ByteRange {
            start,
            end: content.len(),
        });
    }

    ranges
}

/// Check if a byte position is within a div block
pub fn is_within_div_block_ranges(ranges: &[ByteRange], position: usize) -> bool {
    ranges.iter().any(|r| position >= r.start && position < r.end)
}

// ============================================================================
// Citation Support
// ============================================================================
//
// Pandoc citation syntax:
// - Inline citation: @smith2020
// - Parenthetical citation: [@smith2020]
// - Suppress author: [-@smith2020]
// - With locator: [@smith2020, p. 10]
// - Multiple citations: [@smith2020; @jones2021]
// - With prefix: [see @smith2020]
//
// Citation keys must start with a letter, digit, or underscore, and may contain
// alphanumerics, underscores, hyphens, periods, and colons.

/// Pattern to match bracketed citations: [@key], [-@key], [see @key], [@a; @b]
static BRACKETED_CITATION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Matches [...] containing at least one @key
    Regex::new(r"\[[^\]]*@[a-zA-Z0-9_][a-zA-Z0-9_:.#$%&\-+?<>~/]*[^\]]*\]").unwrap()
});

/// Pattern to match inline citations: @key (not inside brackets)
/// Citation key: starts with letter/digit/underscore, contains alphanumerics and some punctuation
/// The @ must be preceded by whitespace, start of line, or punctuation (not alphanumeric)
static INLINE_CITATION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Match @ at start of string, after whitespace, or after non-alphanumeric (except @[)
    Regex::new(r"(?:^|[\s\(\[\{,;:])(@[a-zA-Z0-9_][a-zA-Z0-9_:.#$%&\-+?<>~/]*)").unwrap()
});

/// Quick check if text might contain citations
#[inline]
pub fn has_citations(text: &str) -> bool {
    text.contains('@')
}

// ============================================================================
// Inline Footnote Support
// ============================================================================
//
// Pandoc inline footnote syntax: ^[footnote text]
//
// The `^` must not be preceded by `!` (image) or by a word character
// (superscript syntax: `2^10^`). The footnote body extends to the first
// unescaped `]`; nested brackets are not supported in this detector.

/// Pattern for Pandoc inline footnotes: `^[note text]`.
/// The `^` must not be preceded by `!` (which would be an image) or by
/// alphanumeric (which would be a superscript: `2^10^`).
static INLINE_FOOTNOTE_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^\w!])(\^\[[^\]]*\])").unwrap());

/// Compute the Pandoc-style slug for a heading text.
///
/// Pandoc's `auto_identifiers` extension:
/// 1. Remove all formatting, links, etc.
/// 2. Remove all footnotes.
/// 3. Remove all non-alphanumeric characters except `_`, `-`, `.`.
/// 4. Replace all spaces with `-`.
/// 5. Lowercase letters.
/// 6. If nothing remains, use `section`.
pub fn pandoc_header_slug(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
            for lc in c.to_lowercase() {
                s.push(lc);
            }
        } else if c.is_whitespace() {
            // Collapse runs of whitespace to a single `-`.
            if !s.ends_with('-') {
                s.push('-');
            }
        }
        // Drop other punctuation entirely.
    }
    let trimmed = s.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "section".to_string()
    } else {
        trimmed
    }
}

/// Find headings in the document and return a set of their Pandoc slugs.
///
/// Scans ATX-style headings (lines beginning with one or more `#`) and computes
/// a slug for each using [`pandoc_header_slug`]. The resulting set is used by
/// the `implicit_header_references` extension detector in [`LintContext`].
///
/// Lines inside fenced code blocks (delimited by ` ``` ` or `~~~`, >= 3 chars)
/// are skipped so that bash comments and shebang lines are not mistaken for
/// headings.
pub fn collect_pandoc_header_slugs(content: &str) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let mut slugs = HashSet::new();
    let mut in_fence = false;
    let mut fence_marker: Option<char> = None;
    for line in content.lines() {
        let trimmed = line.trim_start();
        // Detect fenced code block open/close. Pandoc fences are >= 3 backticks
        // or >= 3 tildes at the start of a line (after optional indentation).
        // A closing fence must use the same marker character as the opening one.
        if let Some(c) = trimmed.chars().next()
            && (c == '`' || c == '~')
        {
            let count = trimmed.chars().take_while(|&ch| ch == c).count();
            if count >= 3 {
                match fence_marker {
                    None => {
                        in_fence = true;
                        fence_marker = Some(c);
                    }
                    Some(m) if m == c => {
                        in_fence = false;
                        fence_marker = None;
                    }
                    _ => {}
                }
                continue;
            }
        }
        if in_fence {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let mut text = rest.trim_start_matches('#').trim();
            // Strip trailing `{#id .class}` attribute block only when the `{...}`
            // extends to the end of the text (possibly followed by whitespace).
            // This prevents `{` appearing inside heading body text (e.g.
            // `# Some {curly} word`) from being mistaken for an attribute block.
            if let Some(idx) = text.rfind(" {")
                && let Some(close_rel) = text[idx + 2..].find('}')
                && text[idx + 2 + close_rel + 1..].trim().is_empty()
            {
                text = &text[..idx];
            }
            slugs.insert(pandoc_header_slug(text));
        }
    }
    slugs
}

// ============================================================================
// Subscript and Superscript Support
// ============================================================================
//
// Pandoc `subscript` extension: `~x~` where x contains no whitespace or `~`.
// Pandoc `superscript` extension: `^x^` where x contains no whitespace or `^`.
//
// These are distinct from GFM strikethrough (`~~text~~`) and Pandoc inline
// footnotes (`^[...]`). The disambiguation rule for subscript is: reject any
// match where the opening or closing `~` is immediately adjacent to another `~`
// (which would make it GFM strikethrough). For superscript, reject matches
// where a `^` neighbour would form `^^`.

/// Pattern for Pandoc subscript: `~x~` where x is non-whitespace, non-`~`.
static SUBSCRIPT_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"~[^\s~]+~").unwrap());

/// Pattern for Pandoc superscript: `^x^` where x is non-whitespace, non-`^`.
static SUPERSCRIPT_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\^[^\s^]+\^").unwrap());

/// Detect Pandoc subscript (`~x~`) and superscript (`^x^`) ranges.
///
/// Returns byte ranges covering the full delimited span (including the
/// delimiter characters). Excludes `~~strikethrough~~` and superscript-like
/// runs of `^^`. The returned ranges are sorted by `start`.
///
/// Note: a `^[…]^` construct will also match `detect_inline_footnote_ranges`.
/// Rules that distinguish footnotes from superscripts must check both accessors.
pub fn detect_subscript_superscript_ranges(content: &str) -> Vec<ByteRange> {
    let bytes = content.as_bytes();
    let mut ranges = Vec::new();

    for m in SUBSCRIPT_PATTERN.find_iter(content) {
        // Reject if preceded or followed by `~` (would be strikethrough).
        let prev = m.start().checked_sub(1).map(|i| bytes[i]).unwrap_or(0);
        let next = bytes.get(m.end()).copied().unwrap_or(0);
        if prev != b'~' && next != b'~' {
            ranges.push(ByteRange {
                start: m.start(),
                end: m.end(),
            });
        }
    }
    for m in SUPERSCRIPT_PATTERN.find_iter(content) {
        // Reject if preceded or followed by `^` (would be a `^^` run).
        let prev = m.start().checked_sub(1).map(|i| bytes[i]).unwrap_or(0);
        let next = bytes.get(m.end()).copied().unwrap_or(0);
        if prev != b'^' && next != b'^' {
            ranges.push(ByteRange {
                start: m.start(),
                end: m.end(),
            });
        }
    }
    // Sort because the two regex passes are merged and their results may interleave.
    ranges.sort_by_key(|r| r.start);
    ranges
}

// ============================================================================
// Example List Support
// ============================================================================
//
// Pandoc `example_lists` extension:
// - Line-start marker: `(@)` or `(@label)` followed by whitespace
// - Inline reference: `(@label)` appearing mid-paragraph (not at line start)
//
// Example keys contain letters, digits, underscores, and hyphens.
// The anonymous form `(@)` is valid as a marker but cannot appear as a reference
// (references require a label to be named).

/// Pattern for an example-list marker at line start: `(@)` or `(@label)` followed
/// by whitespace. Captures the `(@...)` portion.
static EXAMPLE_LIST_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*(\(@[A-Za-z0-9_-]*\))[ \t]+").unwrap());

/// Pattern for an example reference: `(@label)` anywhere in text. Used together
/// with the marker pre-pass to filter out line-start markers.
static EXAMPLE_REFERENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\(@[A-Za-z0-9_-]+\))").unwrap());

/// Detect Pandoc example-list marker ranges (`(@)` / `(@label)` at line start).
///
/// Returns byte ranges covering the `(@...)` portion of each marker. Used by
/// rules that process list markers to skip Pandoc example markers.
pub fn detect_example_list_marker_ranges(content: &str) -> Vec<ByteRange> {
    let mut ranges = Vec::new();
    for caps in EXAMPLE_LIST_MARKER.captures_iter(content) {
        let m = caps.get(1).unwrap();
        ranges.push(ByteRange {
            start: m.start(),
            end: m.end(),
        });
    }
    ranges
}

/// Detect Pandoc example reference ranges (`(@label)` not at line start).
///
/// Excludes positions whose start byte appears in `marker_ranges` (those are
/// line-start markers, not references). The caller must pass the already-computed
/// result of [`detect_example_list_marker_ranges`] so the marker regex is not
/// executed a second time.
pub fn detect_example_reference_ranges(content: &str, marker_ranges: &[ByteRange]) -> Vec<ByteRange> {
    let mut ranges = Vec::new();
    let marker_starts: std::collections::HashSet<usize> = marker_ranges.iter().map(|r| r.start).collect();
    for caps in EXAMPLE_REFERENCE.captures_iter(content) {
        let m = caps.get(1).unwrap();
        if !marker_starts.contains(&m.start()) {
            ranges.push(ByteRange {
                start: m.start(),
                end: m.end(),
            });
        }
    }
    ranges
}

/// Detect Pandoc inline footnote ranges (`^[note text]`).
///
/// Returns byte ranges covering the entire `^[...]` span. Intended for rules that
/// process bracket-like syntax to skip Pandoc inline footnotes.
pub fn detect_inline_footnote_ranges(content: &str) -> Vec<ByteRange> {
    let mut ranges = Vec::new();
    for caps in INLINE_FOOTNOTE_PATTERN.captures_iter(content) {
        let m = caps.get(1).unwrap();
        ranges.push(ByteRange {
            start: m.start(),
            end: m.end(),
        });
    }
    ranges
}

/// Find all citation ranges in content (byte ranges)
/// Returns ranges for both bracketed `[@key]` and inline `@key` citations
pub fn find_citation_ranges(content: &str) -> Vec<ByteRange> {
    let mut ranges = Vec::new();

    // Find bracketed citations first (higher priority)
    for mat in BRACKETED_CITATION_PATTERN.find_iter(content) {
        ranges.push(ByteRange {
            start: mat.start(),
            end: mat.end(),
        });
    }

    // Find inline citations (but not inside already-found brackets)
    for cap in INLINE_CITATION_PATTERN.captures_iter(content) {
        if let Some(mat) = cap.get(1) {
            let start = mat.start();
            // Skip if this is inside a bracketed citation
            if !ranges.iter().any(|r| start >= r.start && start < r.end) {
                ranges.push(ByteRange { start, end: mat.end() });
            }
        }
    }

    // Sort by start position
    ranges.sort_by_key(|r| r.start);
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_div_open_detection() {
        // Valid div openings
        assert!(is_div_open("::: {.callout-note}"));
        assert!(is_div_open("::: {.callout-warning}"));
        assert!(is_div_open("::: {#myid .class}"));
        assert!(is_div_open("::: bordered"));
        assert!(is_div_open("  ::: {.note}")); // Indented
        assert!(is_div_open("::: {.callout-tip title=\"My Title\"}"));

        // Invalid patterns
        assert!(!is_div_open(":::")); // Just closing marker
        assert!(!is_div_open(":::  ")); // Just closing with trailing space
        assert!(!is_div_open("Regular text"));
        assert!(!is_div_open("# Heading"));
        assert!(!is_div_open("```python")); // Code fence
    }

    #[test]
    fn test_div_close_detection() {
        assert!(is_div_close(":::"));
        assert!(is_div_close(":::  "));
        assert!(is_div_close("  :::"));
        assert!(is_div_close("    :::  "));

        assert!(!is_div_close("::: {.note}"));
        assert!(!is_div_close("::: class"));
        assert!(!is_div_close(":::note"));
    }

    #[test]
    fn test_callout_detection() {
        assert!(is_callout_open("::: {.callout-note}"));
        assert!(is_callout_open("::: {.callout-warning}"));
        assert!(is_callout_open("::: {.callout-tip}"));
        assert!(is_callout_open("::: {.callout-important}"));
        assert!(is_callout_open("::: {.callout-caution}"));
        assert!(is_callout_open("::: {#myid .callout-note}"));
        assert!(is_callout_open("::: {.callout-note title=\"Title\"}"));

        assert!(!is_callout_open("::: {.note}")); // Not a callout
        assert!(!is_callout_open("::: {.bordered}")); // Not a callout
        assert!(!is_callout_open("::: callout-note")); // Missing braces
    }

    #[test]
    fn test_div_tracker() {
        let mut tracker = DivTracker::new();

        // Enter a div
        assert!(tracker.process_line("::: {.callout-note}"));
        assert!(tracker.is_inside_div());

        // Inside content
        assert!(tracker.process_line("This is content."));
        assert!(tracker.is_inside_div());

        // Exit the div
        assert!(!tracker.process_line(":::"));
        assert!(!tracker.is_inside_div());
    }

    #[test]
    fn test_nested_divs() {
        let mut tracker = DivTracker::new();

        // Outer div
        assert!(tracker.process_line("::: {.outer}"));
        assert!(tracker.is_inside_div());

        // Inner div
        assert!(tracker.process_line("  ::: {.inner}"));
        assert!(tracker.is_inside_div());

        // Content
        assert!(tracker.process_line("    Content"));
        assert!(tracker.is_inside_div());

        // Close inner
        assert!(tracker.process_line("  :::"));
        assert!(tracker.is_inside_div());

        // Close outer
        assert!(!tracker.process_line(":::"));
        assert!(!tracker.is_inside_div());
    }

    #[test]
    fn test_detect_div_block_ranges() {
        let content = r#"# Heading

::: {.callout-note}
This is a note.
:::

Regular text.

::: {.bordered}
Content here.
:::
"#;
        let ranges = detect_div_block_ranges(content);
        assert_eq!(ranges.len(), 2);

        // First div
        let first_div_content = &content[ranges[0].start..ranges[0].end];
        assert!(first_div_content.contains("callout-note"));
        assert!(first_div_content.contains("This is a note"));

        // Second div
        let second_div_content = &content[ranges[1].start..ranges[1].end];
        assert!(second_div_content.contains("bordered"));
        assert!(second_div_content.contains("Content here"));
    }

    #[test]
    fn test_pandoc_attributes() {
        assert!(has_pandoc_attributes("# Heading {#custom-id}"));
        assert!(has_pandoc_attributes("# Heading {.unnumbered}"));
        assert!(has_pandoc_attributes("![Image](path.png){#fig-1 width=\"50%\"}"));
        assert!(has_pandoc_attributes("{#id .class key=\"value\"}"));

        assert!(!has_pandoc_attributes("# Heading"));
        assert!(!has_pandoc_attributes("Regular text"));
        assert!(!has_pandoc_attributes("{}"));
    }

    #[test]
    fn test_div_with_title_attribute() {
        let content = r#"::: {.callout-note title="Important Note"}
This is the content of the note.
It can span multiple lines.
:::
"#;
        let ranges = detect_div_block_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert!(is_callout_open("::: {.callout-note title=\"Important Note\"}"));
    }

    #[test]
    fn test_unclosed_div() {
        let content = r#"::: {.callout-note}
This note is never closed.
"#;
        let ranges = detect_div_block_ranges(content);
        assert_eq!(ranges.len(), 1);
        // Should include all content to end of document
        assert_eq!(ranges[0].end, content.len());
    }

    #[test]
    fn test_heading_inside_callout() {
        let content = r#"::: {.callout-warning}
## Warning Title

Warning content here.
:::
"#;
        let ranges = detect_div_block_ranges(content);
        assert_eq!(ranges.len(), 1);

        let div_content = &content[ranges[0].start..ranges[0].end];
        assert!(div_content.contains("## Warning Title"));
    }

    // Citation tests
    #[test]
    fn test_has_citations() {
        assert!(has_citations("See @smith2020 for details."));
        assert!(has_citations("[@smith2020]"));
        assert!(has_citations("Multiple [@a; @b] citations"));
        assert!(!has_citations("No citations here"));
        // has_citations is just a quick @ check - emails will pass (intended behavior)
        assert!(has_citations("Email: user@example.com"));
    }

    #[test]
    fn test_bracketed_citation_detection() {
        let content = "See [@smith2020] for more info.";
        let ranges = find_citation_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "[@smith2020]");
    }

    #[test]
    fn test_inline_citation_detection() {
        let content = "As @smith2020 argues, this is true.";
        let ranges = find_citation_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "@smith2020");
    }

    #[test]
    fn test_multiple_citations_in_brackets() {
        let content = "See [@smith2020; @jones2021] for details.";
        let ranges = find_citation_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "[@smith2020; @jones2021]");
    }

    #[test]
    fn test_citation_with_prefix() {
        let content = "[see @smith2020, p. 10]";
        let ranges = find_citation_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "[see @smith2020, p. 10]");
    }

    #[test]
    fn test_suppress_author_citation() {
        let content = "The theory [-@smith2020] states that...";
        let ranges = find_citation_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "[-@smith2020]");
    }

    #[test]
    fn test_mixed_citations() {
        let content = "@smith2020 argues that [@jones2021] is wrong.";
        let ranges = find_citation_ranges(content);
        assert_eq!(ranges.len(), 2);
        // Inline citation
        assert_eq!(&content[ranges[0].start..ranges[0].end], "@smith2020");
        // Bracketed citation
        assert_eq!(&content[ranges[1].start..ranges[1].end], "[@jones2021]");
    }

    #[test]
    fn test_email_not_confused_with_citation() {
        // Email addresses should not match as inline citations when properly filtered
        // The has_citations() is just a quick check, but find_citation_ranges uses more strict patterns
        let content = "Contact user@example.com for help.";
        let ranges = find_citation_ranges(content);
        // Email should not be detected as citation (@ is preceded by alphanumeric)
        assert!(
            ranges.is_empty()
                || !ranges.iter().any(|r| {
                    let s = &content[r.start..r.end];
                    s.contains("example.com")
                })
        );
    }

    #[test]
    fn test_detect_inline_footnotes() {
        let content = "See ^[a quick note] here.\nAnd ^[another one] too.\n";
        let ranges = detect_inline_footnote_ranges(content);
        assert_eq!(ranges.len(), 2);
        // First footnote
        let first_start = content.find("^[").unwrap();
        let first_end = content[first_start..].find(']').unwrap() + first_start + 1;
        assert_eq!(ranges[0].start, first_start);
        assert_eq!(ranges[0].end, first_end);
        // Second footnote
        let second_start = content[first_end..].find("^[").unwrap() + first_end;
        let second_end = content[second_start..].find(']').unwrap() + second_start + 1;
        assert_eq!(ranges[1].start, second_start);
        assert_eq!(ranges[1].end, second_end);
    }

    #[test]
    fn test_inline_footnote_with_brackets_inside() {
        // Inline footnotes do not nest; a `]` inside terminates the footnote.
        // This documents the chosen behavior. Pandoc itself supports nesting via
        // backslash-escapes; rumdl currently treats the first unescaped `]` as
        // the terminator.
        let content = "Note ^[ref to [other] thing] here.\n";
        let ranges = detect_inline_footnote_ranges(content);
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn test_inline_footnote_does_not_match_image_or_link() {
        // `![alt]` is an image, not a footnote.
        let content = "An image ![alt](url) and a link [txt](url).\n";
        let ranges = detect_inline_footnote_ranges(content);
        assert_eq!(ranges.len(), 0);
    }

    #[test]
    fn test_implicit_header_reference_slug() {
        // Pandoc lowercases, replaces internal whitespace with `-`, and strips
        // punctuation other than `_`, `-`, `.`.
        assert_eq!(pandoc_header_slug("My Section"), "my-section");
        assert_eq!(pandoc_header_slug("API: v2!"), "api-v2");
        assert_eq!(pandoc_header_slug("  Trim Me  "), "trim-me");
        assert_eq!(pandoc_header_slug("Multiple   Spaces"), "multiple-spaces");
    }

    #[test]
    fn test_collect_pandoc_header_slugs() {
        let content = "# My Section\n\n## Sub-section\n\nbody\n";
        let slugs = collect_pandoc_header_slugs(content);
        assert!(slugs.contains("my-section"));
        assert!(slugs.contains("sub-section"));
    }

    #[test]
    fn test_collect_pandoc_header_slugs_strips_attribute_block() {
        let content = "# My Section {#custom-id .red}\n## Plain Section\n";
        let slugs = collect_pandoc_header_slugs(content);
        assert!(slugs.contains("my-section"));
        assert!(slugs.contains("plain-section"));
        // Slug must not include the attribute block contents.
        assert!(!slugs.iter().any(|s| s.contains("custom-id")));
    }

    #[test]
    fn test_collect_pandoc_header_slugs_preserves_body_braces() {
        // `{` in heading body must NOT be mistaken for an attribute block.
        let content = "# Some {curly} word in title\n";
        let slugs = collect_pandoc_header_slugs(content);
        assert!(slugs.contains("some-curly-word-in-title"));
    }

    #[test]
    fn test_detect_example_list_markers() {
        let content = "(@)  First item.\n(@good) Second item.\n(@) Third item.\n";
        let ranges = detect_example_list_marker_ranges(content);
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0].start, 0);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "(@)");
        let second_start = content.find("(@good)").unwrap();
        assert_eq!(ranges[1].start, second_start);
        assert_eq!(&content[ranges[1].start..ranges[1].end], "(@good)");
    }

    #[test]
    fn test_detect_example_references() {
        // `(@label)` mid-paragraph is a reference, not a list marker.
        let content = "As shown in (@good), this works.\n";
        let marker_ranges = detect_example_list_marker_ranges(content);
        let ranges = detect_example_reference_ranges(content, &marker_ranges);
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn test_example_marker_must_be_at_line_start() {
        let content = "Inline (@) is not a marker.\n";
        let ranges = detect_example_list_marker_ranges(content);
        assert_eq!(ranges.len(), 0);
    }

    #[test]
    fn test_detect_subscript() {
        let content = "H~2~O is water.\n";
        let ranges = detect_subscript_superscript_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "~2~");
    }

    #[test]
    fn test_detect_superscript() {
        let content = "2^10^ is 1024.\n";
        let ranges = detect_subscript_superscript_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "^10^");
    }

    #[test]
    fn test_subscript_does_not_match_strikethrough() {
        // `~~text~~` is GFM strikethrough, not subscript.
        let content = "This is ~~struck~~.\n";
        let ranges = detect_subscript_superscript_ranges(content);
        assert_eq!(ranges.len(), 0);
    }

    #[test]
    fn test_superscript_with_internal_space_is_not_matched() {
        // Pandoc requires no whitespace inside `^...^`.
        let content = "x^a b^ y\n";
        let ranges = detect_subscript_superscript_ranges(content);
        assert_eq!(ranges.len(), 0);
    }

    #[test]
    fn test_subscript_at_start_of_input() {
        // Position 0: previous-byte path uses checked_sub(1).unwrap_or(0).
        let content = "~x~ rest of line\n";
        let ranges = detect_subscript_superscript_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "~x~");
    }

    #[test]
    fn test_superscript_at_end_of_input_no_newline() {
        // EOF: next-byte path uses bytes.get(end).unwrap_or(0).
        let content = "text ^x^";
        let ranges = detect_subscript_superscript_ranges(content);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&content[ranges[0].start..ranges[0].end], "^x^");
    }

    #[test]
    fn test_collect_pandoc_header_slugs_skips_code_blocks() {
        let content = "\
# Real Heading

```bash
# This is a bash comment
#!/usr/bin/env bash
```

# Another Heading
";
        let slugs = collect_pandoc_header_slugs(content);
        assert!(slugs.contains("real-heading"));
        assert!(slugs.contains("another-heading"));
        assert!(!slugs.contains("this-is-a-bash-comment"));
        assert!(!slugs.iter().any(|s| s.contains("usr-bin")));
    }
}
