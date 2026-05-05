use rumdl_lib::config::MarkdownFlavor;
use rumdl_lib::lint_context::LintContext;
use rumdl_lib::rule::Rule;
use rumdl_lib::rules::MD031BlanksAroundFences;
use rumdl_lib::rules::{CodeBlockStyle, MD046CodeBlockStyle};

fn azure_ctx(content: &str) -> LintContext<'_> {
    LintContext::new(content, MarkdownFlavor::AzureDevOps, None)
}

#[test]
fn test_md031_colon_fence_missing_blank_before() {
    let content = "Some text\n::: mermaid\ndiagram\n:::\n";
    let ctx = azure_ctx(content);
    let rule = MD031BlanksAroundFences::new(true);
    let warnings = rule.check(&ctx).unwrap();
    assert!(!warnings.is_empty(), "should warn: no blank line before colon fence");
    assert!(
        warnings[0].message.contains("before"),
        "message: {:?}",
        warnings[0].message
    );
}

#[test]
fn test_md031_colon_fence_missing_blank_after() {
    let content = "::: mermaid\ndiagram\n:::\nSome text\n";
    let ctx = azure_ctx(content);
    let rule = MD031BlanksAroundFences::new(true);
    let warnings = rule.check(&ctx).unwrap();
    assert!(!warnings.is_empty(), "should warn: no blank line after colon fence");
    assert!(
        warnings[0].message.contains("after"),
        "message: {:?}",
        warnings[0].message
    );
}

#[test]
fn test_md031_colon_fence_with_blank_lines_no_warning() {
    let content = "Some text\n\n::: mermaid\ndiagram\n:::\n\nSome text\n";
    let ctx = azure_ctx(content);
    let rule = MD031BlanksAroundFences::new(true);
    let warnings = rule.check(&ctx).unwrap();
    assert!(
        warnings.is_empty(),
        "should not warn when blank lines present: {warnings:?}"
    );
}

#[test]
fn test_md031_standard_flavor_colon_not_enforced() {
    let content = "Some text\n::: mermaid\ndiagram\n:::\nSome text\n";
    let ctx = LintContext::new(content, MarkdownFlavor::Standard, None);
    let rule = MD031BlanksAroundFences::new(true);
    let warnings = rule.check(&ctx).unwrap();
    for w in &warnings {
        assert!(!w.message.contains("colon"), "unexpected warning: {w:?}");
    }
}

#[test]
fn test_md046_colon_fence_with_inner_backtick_not_counted() {
    // A colon fence that contains ``` inside should not affect MD046 style detection
    let content = "::: mermaid\n```\nsome content\n```\n:::\n\n```rust\nfn main() {}\n```\n";
    let ctx = azure_ctx(content);
    let rule = MD046CodeBlockStyle::new(CodeBlockStyle::Consistent);
    let warnings = rule.check(&ctx).unwrap();
    assert!(
        warnings.is_empty(),
        "MD046 should not flag colon fence content: {warnings:?}"
    );
}
