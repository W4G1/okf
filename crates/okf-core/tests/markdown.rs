use okf_core::markdown::{
    LinkRewriteAction, clean_destination, extract_headings, extract_title_suffix, heading_slug,
    matches_heading, parse_heading_line, rewrite_markdown_links, strip_title,
};

#[test]
fn test_heading_slug_derivation() {
    assert_eq!(heading_slug("Overview"), "overview");
    assert_eq!(heading_slug("## Deep Section"), "deep-section");
    assert_eq!(
        heading_slug("### Cost & Revenue Analysis (2026)"),
        "cost-revenue-analysis-2026"
    );
    assert_eq!(heading_slug("trailing-dash---"), "trailing-dash");
    assert_eq!(
        heading_slug("multiple   spaces___and---dashes"),
        "multiple-spaces-and-dashes"
    );
}

#[test]
fn test_parse_heading_line() {
    assert_eq!(parse_heading_line("# Level One"), Some((1, "Level One")));
    assert_eq!(parse_heading_line("## Level Two"), Some((2, "Level Two")));
    assert_eq!(
        parse_heading_line("###### Level Six"),
        Some((6, "Level Six"))
    );
    assert_eq!(parse_heading_line("####### Too Deep"), None);
    assert_eq!(parse_heading_line("#NoSpace"), None);
    assert_eq!(parse_heading_line("Not a heading"), None);
}

#[test]
fn test_extract_headings_skips_code_blocks() {
    let body = "\
# Main Title

```
# Code Block Heading
```

## Section 1

~~~python
### Python Heading
~~~

### Section 1.1
";
    let headings = extract_headings(body);
    assert_eq!(headings.len(), 3);
    assert_eq!(headings[0].level, 1);
    assert_eq!(headings[0].text, "Main Title");
    assert_eq!(headings[0].slug(), "main-title");

    assert_eq!(headings[1].level, 2);
    assert_eq!(headings[1].text, "Section 1");
    assert_eq!(headings[1].slug(), "section-1");

    assert_eq!(headings[2].level, 3);
    assert_eq!(headings[2].text, "Section 1.1");
    assert_eq!(headings[2].slug(), "section-11");
}

#[test]
fn test_matches_heading() {
    assert!(matches_heading("## Pricing Model", "Pricing Model"));
    assert!(matches_heading("pricing_model", "pricing-model"));
    assert!(matches_heading("Pricing-Model", "pricing model"));
    assert!(matches_heading("### Advanced Setup", "advanced-setup"));
    assert!(!matches_heading("Setup", "Different"));
}

#[test]
fn test_clean_destination_and_title_stripping() {
    assert_eq!(
        clean_destination("<path with spaces/file.md>"),
        "path with spaces/file.md"
    );
    assert_eq!(clean_destination("guide.md \"Guide Title\""), "guide.md");
    assert_eq!(clean_destination("guide.md 'Guide Title'"), "guide.md");
    assert_eq!(strip_title("guide.md \"Guide Title\""), "guide.md");
    assert_eq!(
        extract_title_suffix("guide.md \"Guide Title\""),
        " \"Guide Title\""
    );
}

#[test]
fn test_rewrite_markdown_links() {
    let body = "\
Check [Token Guide](auth/token.md \"Token\") and [Invoice](../billing/invoice.md).
Do not change `[Code](stay.md)` or:

```
[Inside](stay.md)
```
";
    let (rewritten, count) = rewrite_markdown_links(body, |link, _| {
        if link.target.contains("auth/token.md") {
            LinkRewriteAction::Rewrite("security/token.md".to_string())
        } else if link.target.contains("invoice.md") {
            LinkRewriteAction::Unlink
        } else {
            LinkRewriteAction::Keep
        }
    });

    assert_eq!(count, 2);
    assert!(rewritten.contains("[Token Guide](security/token.md \"Token\")"));
    assert!(rewritten.contains("and Invoice."));
    assert!(rewritten.contains("`[Code](stay.md)`"));
    assert!(rewritten.contains("[Inside](stay.md)"));
}
