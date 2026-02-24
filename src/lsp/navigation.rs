//! Go-to-definition and find-references for markdown links
//!
//! Provides navigation features for the LSP server:
//!
//! - **Go to definition** -- jump from a `[text](file.md#heading)` link to the
//!   target file and heading.
//!
//! - **Find references** -- from a heading, find all links pointing to it across
//!   the workspace.

use std::path::Path;

use tower_lsp::lsp_types::*;

use super::completion::{normalize_path, utf16_to_byte_offset};
use super::server::RumdlLanguageServer;

/// Full link target extracted from a markdown link `[text](file_path#anchor)`.
///
/// Unlike `LinkTargetInfo` (used for completion, which returns content up to the
/// cursor), this struct contains the complete file path and anchor regardless of
/// where the cursor sits within the link target.
struct FullLinkTarget {
    /// The file path portion (before `#`), may be empty for same-file anchors
    file_path: String,
    /// The anchor/fragment portion (after `#`), empty when absent
    anchor: String,
}

/// Strip a CommonMark link title from a link target.
///
/// Link titles start after whitespace followed by `"`, `'`, or `(`.
/// E.g., `guide.md "My Title"` -> `guide.md`
fn strip_link_title(target: &str) -> &str {
    for (i, _) in target.match_indices(' ') {
        let after = &target[i + 1..];
        if after.starts_with('"') || after.starts_with('\'') || after.starts_with('(') {
            return target[..i].trim_end();
        }
    }
    target
}

/// Detect the full link target when the cursor is anywhere inside `](...)`.
///
/// Scans backward from the cursor to find `](` and forward to find the closing
/// `)`, then extracts the complete file path and optional anchor.
fn detect_full_link_target(text: &str, position: Position) -> Option<FullLinkTarget> {
    let line_num = position.line as usize;
    let utf16_cursor = position.character as usize;

    let lines: Vec<&str> = text.lines().collect();
    if line_num >= lines.len() {
        return None;
    }
    let line = lines[line_num];

    // Convert UTF-16 cursor to byte offset
    let byte_cursor = utf16_to_byte_offset(line, utf16_cursor)?;

    let before_cursor = &line[..byte_cursor];

    // Find the last `](` before the cursor
    let link_open = before_cursor.rfind("](")?;
    let content_start = link_open + 2;

    // Find the closing `)` after the content start
    let after_open = &line[content_start..];
    let close_paren = after_open.find(')')?;

    let raw_content = &after_open[..close_paren];

    // Verify the cursor is within the link target (between `](` and `)`)
    let content_end = content_start + close_paren;
    if byte_cursor < content_start || byte_cursor > content_end {
        return None;
    }

    // Heuristic: odd number of backticks before `](` suggests code span
    let backtick_count = before_cursor[..link_open].chars().filter(|&c| c == '`').count();
    if backtick_count % 2 != 0 {
        return None;
    }

    // Strip angle brackets: [text](<path.md>) -> path.md
    let content = raw_content.trim();
    let content = if content.starts_with('<') && content.ends_with('>') {
        &content[1..content.len() - 1]
    } else {
        content
    };

    // Strip link title: guide.md "Title" -> guide.md
    let content = strip_link_title(content);

    // Split on first `#` to separate file path from anchor
    if let Some(hash_pos) = content.find('#') {
        Some(FullLinkTarget {
            file_path: content[..hash_pos].to_string(),
            anchor: content[hash_pos + 1..].to_string(),
        })
    } else {
        Some(FullLinkTarget {
            file_path: content.to_string(),
            anchor: String::new(),
        })
    }
}

/// Find same-file fragment-only links (e.g., `[text](#anchor)`) in the given content.
///
/// Scans the document content for `](#anchor)` patterns and returns locations where
/// the anchor matches (case-insensitive).
fn find_same_file_fragment_links(content: &str, uri: &Url, anchor: &str) -> Vec<Location> {
    let mut locations = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let mut search_from = 0;
        while let Some(pos) = line[search_from..].find("](#") {
            let abs_pos = search_from + pos;
            let after_hash = &line[abs_pos + 3..];
            // Extract anchor up to closing `)`
            if let Some(close) = after_hash.find(')') {
                let link_anchor = &after_hash[..close];
                if link_anchor.eq_ignore_ascii_case(anchor) {
                    let character = abs_pos as u32;
                    locations.push(Location {
                        uri: uri.clone(),
                        range: Range {
                            start: Position {
                                line: line_idx as u32,
                                character,
                            },
                            end: Position {
                                line: line_idx as u32,
                                character,
                            },
                        },
                    });
                }
                search_from = abs_pos + 3 + close;
            } else {
                break;
            }
        }
    }
    locations
}

impl RumdlLanguageServer {
    /// Handle `textDocument/definition` requests.
    ///
    /// When the cursor is on a markdown link `[text](target.md#anchor)`, resolves
    /// the target file path and optional heading anchor, then returns a `Location`
    /// pointing to the target.
    pub(super) async fn handle_goto_definition(&self, uri: &Url, position: Position) -> Option<GotoDefinitionResponse> {
        let text = self.get_document_content(uri).await?;

        let link = detect_full_link_target(&text, position)?;

        let current_file = uri.to_file_path().ok()?;
        let current_dir = current_file.parent()?.to_path_buf();

        // Resolve target file: empty path means same-file anchor
        let target_path = if link.file_path.is_empty() {
            current_file.clone()
        } else {
            normalize_path(current_dir.join(&link.file_path))
        };

        let target_uri = Url::from_file_path(&target_path).ok()?;

        // Determine target line from anchor
        let target_line = if link.anchor.is_empty() {
            0
        } else {
            self.resolve_heading_line(&target_path, &link.anchor).await.unwrap_or(0)
        };

        let target_position = Position {
            line: target_line,
            character: 0,
        };
        let range = Range {
            start: target_position,
            end: target_position,
        };

        Some(GotoDefinitionResponse::Scalar(Location { uri: target_uri, range }))
    }

    /// Handle `textDocument/references` requests.
    ///
    /// When the cursor is on a heading, finds all links across the workspace that
    /// reference this heading. When the cursor is on a link, finds all other links
    /// that point to the same target.
    pub(super) async fn handle_references(&self, uri: &Url, position: Position) -> Option<Vec<Location>> {
        let text = self.get_document_content(uri).await?;
        let current_file = uri.to_file_path().ok()?;

        // Check if cursor is on a heading by consulting the workspace index.
        // This avoids false positives from `#` lines inside code blocks.
        let heading_line_1indexed = (position.line as usize) + 1;
        let heading_anchor = {
            let index = self.workspace_index.read().await;
            index.get_file(&current_file).and_then(|file_index| {
                file_index
                    .headings
                    .iter()
                    .find(|h| h.line == heading_line_1indexed)
                    .map(|h| h.custom_anchor.clone().unwrap_or_else(|| h.auto_anchor.clone()))
            })
        };

        if let Some(anchor) = heading_anchor {
            // Find cross-file references
            let mut locations = self
                .find_references_to_target(&current_file, &anchor)
                .await
                .unwrap_or_default();

            // Also find same-file fragment-only links (e.g., [text](#anchor))
            let same_file = find_same_file_fragment_links(&text, uri, &anchor);
            locations.extend(same_file);

            return if locations.is_empty() { None } else { Some(locations) };
        }

        // Check if cursor is on a link
        if let Some(link) = detect_full_link_target(&text, position) {
            let current_dir = current_file.parent()?.to_path_buf();
            let target_path = if link.file_path.is_empty() {
                current_file.clone()
            } else {
                normalize_path(current_dir.join(&link.file_path))
            };

            return self.find_references_to_target(&target_path, &link.anchor).await;
        }

        None
    }

    /// Look up a heading's line number (0-indexed for LSP) in the workspace index.
    async fn resolve_heading_line(&self, file_path: &Path, anchor: &str) -> Option<u32> {
        let index = self.workspace_index.read().await;
        let file_index = index.get_file(file_path)?;
        let heading = file_index.get_heading_by_anchor(anchor)?;
        // HeadingIndex.line is 1-indexed; LSP is 0-indexed
        Some((heading.line.saturating_sub(1)) as u32)
    }

    /// Find all links across the workspace that point to `target_path` with
    /// the given `fragment` (anchor).
    ///
    /// An empty fragment matches links that target the file without an anchor.
    async fn find_references_to_target(&self, target_path: &Path, fragment: &str) -> Option<Vec<Location>> {
        let index = self.workspace_index.read().await;
        let mut locations = Vec::new();

        for (source_path, file_index) in index.files() {
            let source_dir = source_path.parent().unwrap_or(Path::new(""));

            for link in &file_index.cross_file_links {
                let resolved_target = normalize_path(source_dir.join(&link.target_path));

                if resolved_target != *target_path {
                    continue;
                }

                let fragment_matches = link.fragment.eq_ignore_ascii_case(fragment);

                if !fragment_matches {
                    continue;
                }

                if let Ok(source_uri) = Url::from_file_path(source_path) {
                    // CrossFileLinkIndex uses 1-indexed line/column; LSP uses 0-indexed
                    let line = (link.line.saturating_sub(1)) as u32;
                    let character = (link.column.saturating_sub(1)) as u32;
                    locations.push(Location {
                        uri: source_uri,
                        range: Range {
                            start: Position { line, character },
                            end: Position { line, character },
                        },
                    });
                }
            }
        }

        if locations.is_empty() { None } else { Some(locations) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_full_link_target_file_only() {
        let text = "See [link](guide.md) here.\n";
        let position = Position { line: 0, character: 14 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
        assert_eq!(link.anchor, "");
    }

    #[test]
    fn test_detect_full_link_target_file_with_anchor() {
        let text = "See [link](guide.md#install) here.\n";
        let position = Position { line: 0, character: 14 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
        assert_eq!(link.anchor, "install");
    }

    #[test]
    fn test_detect_full_link_target_same_file_anchor() {
        let text = "See [below](#configuration) here.\n";
        let position = Position { line: 0, character: 15 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "");
        assert_eq!(link.anchor, "configuration");
    }

    #[test]
    fn test_detect_full_link_target_cursor_outside_link() {
        let text = "Just some text here.\n";
        let position = Position { line: 0, character: 5 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_full_link_target_cursor_at_start_of_target() {
        let text = "See [link](guide.md) here.\n";
        // Cursor right after `](`
        let position = Position { line: 0, character: 11 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
    }

    #[test]
    fn test_detect_full_link_target_cursor_at_end_of_target() {
        let text = "See [link](guide.md) here.\n";
        // Cursor right before `)`
        let position = Position { line: 0, character: 19 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
    }

    #[test]
    fn test_detect_full_link_target_in_code_span() {
        let text = "See `[link](guide.md)` here.\n";
        let position = Position { line: 0, character: 15 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_none(), "Should not detect links inside code spans");
    }

    #[test]
    fn test_detect_full_link_target_with_title() {
        let text = r#"See [link](guide.md "Title") here."#;
        let position = Position { line: 0, character: 14 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
        assert_eq!(link.anchor, "");
    }

    #[test]
    fn test_detect_full_link_target_with_single_quote_title() {
        let text = "See [link](guide.md 'Title') here.";
        let position = Position { line: 0, character: 14 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
    }

    #[test]
    fn test_detect_full_link_target_with_paren_title() {
        let text = "See [link](guide.md (Title)) here.";
        let position = Position { line: 0, character: 14 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
    }

    #[test]
    fn test_detect_full_link_target_with_title_and_anchor() {
        let text = r#"See [link](guide.md#install "Install Guide") here."#;
        let position = Position { line: 0, character: 14 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
        assert_eq!(link.anchor, "install");
    }

    #[test]
    fn test_detect_full_link_target_angle_brackets() {
        let text = "See [link](<guide.md>) here.";
        let position = Position { line: 0, character: 14 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
        assert_eq!(link.anchor, "");
    }

    #[test]
    fn test_detect_full_link_target_angle_brackets_with_anchor() {
        let text = "See [link](<guide.md#install>) here.";
        let position = Position { line: 0, character: 14 };
        let result = detect_full_link_target(text, position);
        assert!(result.is_some());
        let link = result.unwrap();
        assert_eq!(link.file_path, "guide.md");
        assert_eq!(link.anchor, "install");
    }

    #[test]
    fn test_strip_link_title_double_quotes() {
        assert_eq!(strip_link_title(r#"file.md "Title""#), "file.md");
    }

    #[test]
    fn test_strip_link_title_single_quotes() {
        assert_eq!(strip_link_title("file.md 'Title'"), "file.md");
    }

    #[test]
    fn test_strip_link_title_parens() {
        assert_eq!(strip_link_title("file.md (Title)"), "file.md");
    }

    #[test]
    fn test_strip_link_title_no_title() {
        assert_eq!(strip_link_title("file.md"), "file.md");
    }

    #[test]
    fn test_strip_link_title_with_spaces_in_path() {
        // Space not followed by title delimiter should not strip
        assert_eq!(strip_link_title("my file.md"), "my file.md");
    }

    #[test]
    fn test_find_same_file_fragment_links_basic() {
        let content = "# Heading\n\nSee [below](#heading) for details.\n";
        let uri = Url::parse("file:///test.md").unwrap();
        let locations = find_same_file_fragment_links(content, &uri, "heading");
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].range.start.line, 2);
    }

    #[test]
    fn test_find_same_file_fragment_links_case_insensitive() {
        let content = "See [link](#HEADING) here.\n";
        let uri = Url::parse("file:///test.md").unwrap();
        let locations = find_same_file_fragment_links(content, &uri, "heading");
        assert_eq!(locations.len(), 1);
    }

    #[test]
    fn test_find_same_file_fragment_links_multiple() {
        let content = "See [a](#heading) and [b](#heading) here.\n";
        let uri = Url::parse("file:///test.md").unwrap();
        let locations = find_same_file_fragment_links(content, &uri, "heading");
        assert_eq!(locations.len(), 2);
    }

    #[test]
    fn test_find_same_file_fragment_links_no_match() {
        let content = "See [link](#other) here.\n";
        let uri = Url::parse("file:///test.md").unwrap();
        let locations = find_same_file_fragment_links(content, &uri, "heading");
        assert_eq!(locations.len(), 0);
    }
}
