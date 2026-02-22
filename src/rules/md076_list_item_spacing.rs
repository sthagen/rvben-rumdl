use crate::lint_context::LintContext;
use crate::rule::{LintError, LintResult, LintWarning, Rule, Severity};

/// Rule MD076: Enforce consistent blank lines between list items
///
/// See [docs/md076.md](../../docs/md076.md) for full documentation and examples.
///
/// Enforces that the spacing between consecutive list items is consistent
/// within each list: either all gaps have a blank line (loose) or none do (tight).
///
/// ## Configuration
///
/// ```toml
/// [MD076]
/// style = "consistent"  # "loose", "tight", or "consistent" (default)
/// ```
///
/// - `"consistent"` — within each list, all gaps must use the same style (majority wins)
/// - `"loose"` — blank line required between every pair of items
/// - `"tight"` — no blank lines allowed between any items

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ListItemSpacingStyle {
    #[default]
    Consistent,
    Loose,
    Tight,
}

#[derive(Debug, Clone, Default)]
pub struct MD076Config {
    pub style: ListItemSpacingStyle,
}

#[derive(Debug, Clone, Default)]
pub struct MD076ListItemSpacing {
    config: MD076Config,
}

/// Per-block analysis result shared by check() and fix().
struct BlockAnalysis {
    /// 1-indexed line numbers of items at this block's nesting level.
    items: Vec<usize>,
    /// Whether each inter-item gap is loose (has a blank separator line).
    gaps: Vec<bool>,
    /// Whether loose gaps are violations (should have blank lines removed).
    warn_loose_gaps: bool,
    /// Whether tight gaps are violations (should have blank lines inserted).
    warn_tight_gaps: bool,
}

impl MD076ListItemSpacing {
    pub fn new(style: ListItemSpacingStyle) -> Self {
        Self {
            config: MD076Config { style },
        }
    }

    /// Check whether a line is effectively blank, accounting for blockquote markers.
    ///
    /// A line like `>` or `> ` is considered blank in blockquote context even though
    /// its raw content is non-empty.
    fn is_effectively_blank(ctx: &LintContext, line_num: usize) -> bool {
        if let Some(info) = ctx.line_info(line_num) {
            let content = info.content(ctx.content);
            if content.trim().is_empty() {
                return true;
            }
            // In a blockquote, a line containing only markers (e.g., ">", "> ") is blank
            if let Some(ref bq) = info.blockquote {
                return bq.content.trim().is_empty();
            }
            false
        } else {
            false
        }
    }

    /// Determine whether the inter-item gap between two consecutive items is loose.
    ///
    /// Only considers blank lines that are actual inter-item separators: the
    /// consecutive blank lines immediately preceding the next item's marker.
    /// Blank lines within a multi-paragraph item (followed by indented continuation
    /// content) are not counted.
    fn gap_is_loose(ctx: &LintContext, first: usize, next: usize) -> bool {
        if next <= first + 1 {
            return false;
        }
        // The gap is loose if the line immediately before the next item is blank.
        // This correctly ignores blank lines within multi-paragraph items that
        // are followed by continuation content rather than the next item marker.
        Self::is_effectively_blank(ctx, next - 1)
    }

    /// Collect the 1-indexed line numbers of all inter-item blank lines in the gap.
    ///
    /// Walks backwards from the line before `next` collecting consecutive blank lines.
    /// These are the actual separator lines between items, not blank lines within
    /// multi-paragraph items.
    fn inter_item_blanks(ctx: &LintContext, first: usize, next: usize) -> Vec<usize> {
        let mut blanks = Vec::new();
        let mut line_num = next - 1;
        while line_num > first && Self::is_effectively_blank(ctx, line_num) {
            blanks.push(line_num);
            line_num -= 1;
        }
        blanks.reverse();
        blanks
    }

    /// Analyze a single list block to determine which gaps need fixing.
    ///
    /// Returns `None` if the block has fewer than 2 items at its nesting level
    /// or if no gaps violate the configured style.
    fn analyze_block(
        ctx: &LintContext,
        block: &crate::lint_context::types::ListBlock,
        style: &ListItemSpacingStyle,
    ) -> Option<BlockAnalysis> {
        // Only compare items at this block's own nesting level.
        // item_lines may include nested list items (higher marker_column) that belong
        // to a child list — those must not affect spacing analysis.
        let items: Vec<usize> = block
            .item_lines
            .iter()
            .copied()
            .filter(|&line_num| {
                ctx.line_info(line_num)
                    .and_then(|li| li.list_item.as_ref())
                    .map(|item| item.marker_column / 2 == block.nesting_level)
                    .unwrap_or(false)
            })
            .collect();

        if items.len() < 2 {
            return None;
        }

        // Compute whether each inter-item gap is loose (has blank separator).
        let gaps: Vec<bool> = items.windows(2).map(|w| Self::gap_is_loose(ctx, w[0], w[1])).collect();

        let loose_count = gaps.iter().filter(|&&g| g).count();
        let tight_count = gaps.len() - loose_count;

        let (warn_loose_gaps, warn_tight_gaps) = match style {
            ListItemSpacingStyle::Loose => (false, true),
            ListItemSpacingStyle::Tight => (true, false),
            ListItemSpacingStyle::Consistent => {
                if loose_count == 0 || tight_count == 0 {
                    return None; // Already consistent
                }
                // Majority wins; on a tie, prefer loose (warn tight).
                if loose_count >= tight_count {
                    (false, true)
                } else {
                    (true, false)
                }
            }
        };

        Some(BlockAnalysis {
            items,
            gaps,
            warn_loose_gaps,
            warn_tight_gaps,
        })
    }
}

impl Rule for MD076ListItemSpacing {
    fn name(&self) -> &'static str {
        "MD076"
    }

    fn description(&self) -> &'static str {
        "List item spacing should be consistent"
    }

    fn check(&self, ctx: &LintContext) -> LintResult {
        if ctx.content.is_empty() {
            return Ok(Vec::new());
        }

        let mut warnings = Vec::new();

        for block in &ctx.list_blocks {
            let Some(analysis) = Self::analyze_block(ctx, block, &self.config.style) else {
                continue;
            };

            for (i, &is_loose) in analysis.gaps.iter().enumerate() {
                if is_loose && analysis.warn_loose_gaps {
                    // Warn on the first inter-item blank line in this gap.
                    let blanks = Self::inter_item_blanks(ctx, analysis.items[i], analysis.items[i + 1]);
                    if let Some(&blank_line) = blanks.first() {
                        let line_content = ctx
                            .line_info(blank_line)
                            .map(|li| li.content(ctx.content))
                            .unwrap_or("");
                        warnings.push(LintWarning {
                            rule_name: Some(self.name().to_string()),
                            line: blank_line,
                            column: 1,
                            end_line: blank_line,
                            end_column: line_content.len() + 1,
                            message: "Unexpected blank line between list items".to_string(),
                            severity: Severity::Warning,
                            fix: None,
                        });
                    }
                } else if !is_loose && analysis.warn_tight_gaps {
                    // Warn on the next item line (a blank line should precede it).
                    let next_item = analysis.items[i + 1];
                    let line_content = ctx.line_info(next_item).map(|li| li.content(ctx.content)).unwrap_or("");
                    warnings.push(LintWarning {
                        rule_name: Some(self.name().to_string()),
                        line: next_item,
                        column: 1,
                        end_line: next_item,
                        end_column: line_content.len() + 1,
                        message: "Missing blank line between list items".to_string(),
                        severity: Severity::Warning,
                        fix: None,
                    });
                }
            }
        }

        Ok(warnings)
    }

    fn fix(&self, ctx: &LintContext) -> Result<String, LintError> {
        if ctx.content.is_empty() {
            return Ok(ctx.content.to_string());
        }

        // Collect all inter-item blank lines to remove and lines to insert before.
        let mut insert_before: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut remove_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for block in &ctx.list_blocks {
            let Some(analysis) = Self::analyze_block(ctx, block, &self.config.style) else {
                continue;
            };

            for (i, &is_loose) in analysis.gaps.iter().enumerate() {
                if is_loose && analysis.warn_loose_gaps {
                    // Remove ALL inter-item blank lines in this gap
                    for blank_line in Self::inter_item_blanks(ctx, analysis.items[i], analysis.items[i + 1]) {
                        remove_lines.insert(blank_line);
                    }
                } else if !is_loose && analysis.warn_tight_gaps {
                    insert_before.insert(analysis.items[i + 1]);
                }
            }
        }

        if insert_before.is_empty() && remove_lines.is_empty() {
            return Ok(ctx.content.to_string());
        }

        let lines = ctx.raw_lines();
        let mut result: Vec<String> = Vec::with_capacity(lines.len());

        for (i, line) in lines.iter().enumerate() {
            let line_num = i + 1;

            if remove_lines.contains(&line_num) {
                continue;
            }

            if insert_before.contains(&line_num) {
                let bq_prefix = ctx.blockquote_prefix_for_blank_line(i);
                result.push(bq_prefix);
            }

            result.push((*line).to_string());
        }

        let mut output = result.join("\n");
        if ctx.content.ends_with('\n') {
            output.push('\n');
        }
        Ok(output)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn default_config_section(&self) -> Option<(String, toml::Value)> {
        let mut map = toml::map::Map::new();
        let style_str = match self.config.style {
            ListItemSpacingStyle::Consistent => "consistent",
            ListItemSpacingStyle::Loose => "loose",
            ListItemSpacingStyle::Tight => "tight",
        };
        map.insert("style".to_string(), toml::Value::String(style_str.to_string()));
        Some((self.name().to_string(), toml::Value::Table(map)))
    }

    fn from_config(config: &crate::config::Config) -> Box<dyn Rule>
    where
        Self: Sized,
    {
        let style = crate::config::get_rule_config_value::<String>(config, "MD076", "style")
            .unwrap_or_else(|| "consistent".to_string());
        let style = match style.as_str() {
            "loose" => ListItemSpacingStyle::Loose,
            "tight" => ListItemSpacingStyle::Tight,
            _ => ListItemSpacingStyle::Consistent,
        };
        Box::new(Self::new(style))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(content: &str, style: ListItemSpacingStyle) -> Vec<LintWarning> {
        let ctx = LintContext::new(content, crate::config::MarkdownFlavor::Standard, None);
        let rule = MD076ListItemSpacing::new(style);
        rule.check(&ctx).unwrap()
    }

    fn fix(content: &str, style: ListItemSpacingStyle) -> String {
        let ctx = LintContext::new(content, crate::config::MarkdownFlavor::Standard, None);
        let rule = MD076ListItemSpacing::new(style);
        rule.fix(&ctx).unwrap()
    }

    // ── Basic style detection ──────────────────────────────────────────

    #[test]
    fn tight_list_tight_style_no_warnings() {
        let content = "- Item 1\n- Item 2\n- Item 3\n";
        assert!(check(content, ListItemSpacingStyle::Tight).is_empty());
    }

    #[test]
    fn loose_list_loose_style_no_warnings() {
        let content = "- Item 1\n\n- Item 2\n\n- Item 3\n";
        assert!(check(content, ListItemSpacingStyle::Loose).is_empty());
    }

    #[test]
    fn tight_list_loose_style_warns() {
        let content = "- Item 1\n- Item 2\n- Item 3\n";
        let warnings = check(content, ListItemSpacingStyle::Loose);
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().all(|w| w.message.contains("Missing")));
    }

    #[test]
    fn loose_list_tight_style_warns() {
        let content = "- Item 1\n\n- Item 2\n\n- Item 3\n";
        let warnings = check(content, ListItemSpacingStyle::Tight);
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().all(|w| w.message.contains("Unexpected")));
    }

    // ── Consistent mode ────────────────────────────────────────────────

    #[test]
    fn consistent_all_tight_no_warnings() {
        let content = "- Item 1\n- Item 2\n- Item 3\n";
        assert!(check(content, ListItemSpacingStyle::Consistent).is_empty());
    }

    #[test]
    fn consistent_all_loose_no_warnings() {
        let content = "- Item 1\n\n- Item 2\n\n- Item 3\n";
        assert!(check(content, ListItemSpacingStyle::Consistent).is_empty());
    }

    #[test]
    fn consistent_mixed_majority_loose_warns_tight() {
        // 2 loose gaps, 1 tight gap → tight is minority → warn on tight
        let content = "- Item 1\n\n- Item 2\n- Item 3\n\n- Item 4\n";
        let warnings = check(content, ListItemSpacingStyle::Consistent);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("Missing"));
    }

    #[test]
    fn consistent_mixed_majority_tight_warns_loose() {
        // 1 loose gap, 2 tight gaps → loose is minority → warn on loose blank line
        let content = "- Item 1\n\n- Item 2\n- Item 3\n- Item 4\n";
        let warnings = check(content, ListItemSpacingStyle::Consistent);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("Unexpected"));
    }

    #[test]
    fn consistent_tie_prefers_loose() {
        let content = "- Item 1\n\n- Item 2\n- Item 3\n";
        let warnings = check(content, ListItemSpacingStyle::Consistent);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("Missing"));
    }

    // ── Edge cases ─────────────────────────────────────────────────────

    #[test]
    fn single_item_list_no_warnings() {
        let content = "- Only item\n";
        assert!(check(content, ListItemSpacingStyle::Loose).is_empty());
        assert!(check(content, ListItemSpacingStyle::Tight).is_empty());
        assert!(check(content, ListItemSpacingStyle::Consistent).is_empty());
    }

    #[test]
    fn empty_content_no_warnings() {
        assert!(check("", ListItemSpacingStyle::Consistent).is_empty());
    }

    #[test]
    fn ordered_list_tight_gaps_loose_style_warns() {
        let content = "1. First\n2. Second\n3. Third\n";
        let warnings = check(content, ListItemSpacingStyle::Loose);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn task_list_works() {
        let content = "- [x] Task 1\n- [ ] Task 2\n- [x] Task 3\n";
        let warnings = check(content, ListItemSpacingStyle::Loose);
        assert_eq!(warnings.len(), 2);
        let fixed = fix(content, ListItemSpacingStyle::Loose);
        assert_eq!(fixed, "- [x] Task 1\n\n- [ ] Task 2\n\n- [x] Task 3\n");
    }

    #[test]
    fn no_trailing_newline() {
        let content = "- Item 1\n- Item 2";
        let warnings = check(content, ListItemSpacingStyle::Loose);
        assert_eq!(warnings.len(), 1);
        let fixed = fix(content, ListItemSpacingStyle::Loose);
        assert_eq!(fixed, "- Item 1\n\n- Item 2");
    }

    #[test]
    fn two_separate_lists() {
        let content = "- A\n- B\n\nText\n\n1. One\n2. Two\n";
        let warnings = check(content, ListItemSpacingStyle::Loose);
        assert_eq!(warnings.len(), 2);
        let fixed = fix(content, ListItemSpacingStyle::Loose);
        assert_eq!(fixed, "- A\n\n- B\n\nText\n\n1. One\n\n2. Two\n");
    }

    #[test]
    fn no_list_content() {
        let content = "Just a paragraph.\n\nAnother paragraph.\n";
        assert!(check(content, ListItemSpacingStyle::Loose).is_empty());
        assert!(check(content, ListItemSpacingStyle::Tight).is_empty());
    }

    // ── Multi-line and continuation items ──────────────────────────────

    #[test]
    fn continuation_lines_tight_detected() {
        let content = "- Item 1\n  continuation\n- Item 2\n";
        let warnings = check(content, ListItemSpacingStyle::Loose);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("Missing"));
    }

    #[test]
    fn continuation_lines_loose_detected() {
        let content = "- Item 1\n  continuation\n\n- Item 2\n";
        assert!(check(content, ListItemSpacingStyle::Loose).is_empty());
        let warnings = check(content, ListItemSpacingStyle::Tight);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("Unexpected"));
    }

    #[test]
    fn multi_paragraph_item_not_treated_as_inter_item_gap() {
        // Blank line between paragraphs within Item 1 must NOT trigger a warning.
        // Only the blank line immediately before Item 2 is an inter-item separator.
        let content = "- Item 1\n\n  Second paragraph\n\n- Item 2\n";
        // Both gaps are loose (blank before Item 2), so tight should warn once
        let warnings = check(content, ListItemSpacingStyle::Tight);
        assert_eq!(
            warnings.len(),
            1,
            "Should warn only on the inter-item blank, not the intra-item blank"
        );
        // The fix should remove only the inter-item blank (line 4), preserving the
        // multi-paragraph structure
        let fixed = fix(content, ListItemSpacingStyle::Tight);
        assert_eq!(fixed, "- Item 1\n\n  Second paragraph\n- Item 2\n");
    }

    #[test]
    fn multi_paragraph_item_loose_style_no_warnings() {
        // A loose list with multi-paragraph items is already loose — no warnings
        let content = "- Item 1\n\n  Second paragraph\n\n- Item 2\n";
        assert!(check(content, ListItemSpacingStyle::Loose).is_empty());
    }

    // ── Blockquote lists ───────────────────────────────────────────────

    #[test]
    fn blockquote_tight_list_loose_style_warns() {
        let content = "> - Item 1\n> - Item 2\n> - Item 3\n";
        let warnings = check(content, ListItemSpacingStyle::Loose);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn blockquote_loose_list_detected() {
        // A line with only `>` is effectively blank in blockquote context
        let content = "> - Item 1\n>\n> - Item 2\n";
        let warnings = check(content, ListItemSpacingStyle::Tight);
        assert_eq!(warnings.len(), 1, "Blockquote-only line should be detected as blank");
        assert!(warnings[0].message.contains("Unexpected"));
    }

    #[test]
    fn blockquote_loose_list_no_warnings_when_loose() {
        let content = "> - Item 1\n>\n> - Item 2\n";
        assert!(check(content, ListItemSpacingStyle::Loose).is_empty());
    }

    // ── Multiple blank lines ───────────────────────────────────────────

    #[test]
    fn multiple_blanks_all_removed() {
        let content = "- Item 1\n\n\n- Item 2\n";
        let fixed = fix(content, ListItemSpacingStyle::Tight);
        assert_eq!(fixed, "- Item 1\n- Item 2\n");
    }

    #[test]
    fn multiple_blanks_fix_is_idempotent() {
        let content = "- Item 1\n\n\n\n- Item 2\n";
        let fixed_once = fix(content, ListItemSpacingStyle::Tight);
        let fixed_twice = fix(&fixed_once, ListItemSpacingStyle::Tight);
        assert_eq!(fixed_once, fixed_twice);
        assert_eq!(fixed_once, "- Item 1\n- Item 2\n");
    }

    // ── Fix correctness ────────────────────────────────────────────────

    #[test]
    fn fix_adds_blank_lines() {
        let content = "- Item 1\n- Item 2\n- Item 3\n";
        let fixed = fix(content, ListItemSpacingStyle::Loose);
        assert_eq!(fixed, "- Item 1\n\n- Item 2\n\n- Item 3\n");
    }

    #[test]
    fn fix_removes_blank_lines() {
        let content = "- Item 1\n\n- Item 2\n\n- Item 3\n";
        let fixed = fix(content, ListItemSpacingStyle::Tight);
        assert_eq!(fixed, "- Item 1\n- Item 2\n- Item 3\n");
    }

    #[test]
    fn fix_consistent_adds_blank() {
        // 2 loose gaps, 1 tight gap → add blank before Item 3
        let content = "- Item 1\n\n- Item 2\n- Item 3\n\n- Item 4\n";
        let fixed = fix(content, ListItemSpacingStyle::Consistent);
        assert_eq!(fixed, "- Item 1\n\n- Item 2\n\n- Item 3\n\n- Item 4\n");
    }

    #[test]
    fn fix_idempotent_loose() {
        let content = "- Item 1\n- Item 2\n";
        let fixed_once = fix(content, ListItemSpacingStyle::Loose);
        let fixed_twice = fix(&fixed_once, ListItemSpacingStyle::Loose);
        assert_eq!(fixed_once, fixed_twice);
    }

    #[test]
    fn fix_idempotent_tight() {
        let content = "- Item 1\n\n- Item 2\n";
        let fixed_once = fix(content, ListItemSpacingStyle::Tight);
        let fixed_twice = fix(&fixed_once, ListItemSpacingStyle::Tight);
        assert_eq!(fixed_once, fixed_twice);
    }

    // ── Nested lists ───────────────────────────────────────────────────

    #[test]
    fn nested_list_does_not_affect_parent() {
        // Nested items should not trigger warnings for the parent list
        let content = "- Item 1\n  - Nested A\n  - Nested B\n- Item 2\n";
        let warnings = check(content, ListItemSpacingStyle::Tight);
        assert!(
            warnings.is_empty(),
            "Nested items should not cause parent-level warnings"
        );
    }

    // ── Config schema ──────────────────────────────────────────────────

    #[test]
    fn default_config_section_provides_style_key() {
        let rule = MD076ListItemSpacing::new(ListItemSpacingStyle::Consistent);
        let section = rule.default_config_section();
        assert!(section.is_some());
        let (name, value) = section.unwrap();
        assert_eq!(name, "MD076");
        if let toml::Value::Table(map) = value {
            assert!(map.contains_key("style"));
        } else {
            panic!("Expected Table value from default_config_section");
        }
    }
}
