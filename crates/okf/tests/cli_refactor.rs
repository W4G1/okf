//! Integration tests for OKF CLI refactoring commands: `mv`, `rm`, `split`, and `merge`.

mod common;

use common::TempDir;
use std::process::Command;

fn okf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_okf"))
}

#[test]
fn cli_mv_renames_and_rewrites_links() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "auth/token.md",
        "---\ntype: Concept\ntitle: Auth Token\n---\n\n# Auth Token\n\nSee [Profile](../users/profile.md).\n",
    );
    tmp.write(
        "users/profile.md",
        "---\ntype: Concept\ntitle: User Profile\n---\n\n# User Profile\n\nUses [Auth Token](../auth/token.md).\n",
    );

    let output = okf()
        .args([
            "mv",
            "auth/token",
            "security/jwt",
            "--bundle",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("renamed concept auth/token -> security/jwt"));
    assert!(stdout.contains("rewrote 1 incoming link(s)"));
    assert!(stdout.contains("rebased 1 outgoing link(s)"));

    // Check files on disk
    assert!(!tmp.path().join("auth/token.md").exists());
    assert!(tmp.path().join("security/jwt.md").exists());

    let moved_content = tmp.read("security/jwt.md");
    assert!(moved_content.contains("[Profile](../users/profile.md)"));

    let users_content = tmp.read("users/profile.md");
    assert!(users_content.contains("[Auth Token](../security/jwt.md)"));

    let log_content = tmp.read("log.md");
    assert!(log_content.contains("Renamed concept `auth/token` to `security/jwt`"));
}

#[test]
fn cli_rename_alias_and_json_output() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "old_name.md",
        "---\ntype: Concept\ntitle: Old Name\n---\n\n# Old Name\n",
    );

    let output = okf()
        .args([
            "rename",
            "old_name",
            "new_name",
            "--bundle",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(val["status"], "ok");
    assert_eq!(val["source"], "old_name");
    assert_eq!(val["target"], "new_name");
    assert_eq!(val["dry_run"], false);
    assert!(!tmp.path().join("old_name.md").exists());
    assert!(tmp.path().join("new_name.md").exists());
}

#[test]
fn cli_rm_protection_and_redirect() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "deprecated.md",
        "---\ntype: Concept\ntitle: Deprecated\n---\n\n# Deprecated\n",
    );
    tmp.write(
        "replacement.md",
        "---\ntype: Concept\ntitle: Replacement\n---\n\n# Replacement\n",
    );
    tmp.write(
        "guide.md",
        "---\ntype: Concept\ntitle: Guide\n---\n\n# Guide\n\nRead [Deprecated](deprecated.md).\n",
    );

    // 1. Without flags -> should fail
    let output_err = okf()
        .args(["rm", "deprecated", "--bundle", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert_eq!(output_err.status.code(), Some(65));
    let stderr = String::from_utf8(output_err.stderr).unwrap();
    assert!(
        stderr.contains("cannot remove concept 'deprecated' because 1 other concept(s) link to it")
    );

    // 2. With --redirect-to
    let output_redirect = okf()
        .args([
            "rm",
            "deprecated",
            "--redirect-to",
            "replacement",
            "--bundle",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output_redirect.status.success());
    assert!(!tmp.path().join("deprecated.md").exists());

    let guide_content = tmp.read("guide.md");
    assert!(guide_content.contains("[Deprecated](replacement.md)"));
}

#[test]
fn cli_rm_unlink_flag() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "obsolete.md",
        "---\ntype: Concept\ntitle: Obsolete\n---\n\n# Obsolete\n",
    );
    tmp.write(
        "guide.md",
        "---\ntype: Concept\ntitle: Guide\n---\n\n# Guide\n\nSee [Obsolete Guide](obsolete.md) for background.\n",
    );

    let output = okf()
        .args([
            "delete",
            "obsolete",
            "--unlink",
            "--bundle",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(val["unlinked_count"], 1);
    assert!(!tmp.path().join("obsolete.md").exists());

    let guide_content = tmp.read("guide.md");
    assert!(guide_content.contains("See Obsolete Guide for background."));
}

#[test]
fn cli_split_extracts_section() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "billing/pricing.md",
        "---\ntype: Concept\ntitle: Pricing\n---\n\n# Pricing\n\nGeneral overview.\n\n## Enterprise Tier\n\nEnterprise pricing includes dedicated support.\nSLA is 99.99% uptime.\n\n## Community Tier\n\nFree tier.\n",
    );

    let output = okf()
        .args([
            "split",
            "billing/pricing",
            "billing/enterprise",
            "--section",
            "Enterprise Tier",
            "--title",
            "Enterprise Pricing",
            "--bundle",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(val["source"], "billing/pricing");
    assert_eq!(val["target"], "billing/enterprise");
    assert_eq!(val["target_title"], "Enterprise Pricing");

    assert!(tmp.path().join("billing/enterprise.md").exists());
    let ent_content = tmp.read("billing/enterprise.md");
    assert!(ent_content.contains("# Enterprise Pricing"));
    assert!(ent_content.contains("Enterprise pricing includes dedicated support."));

    let pricing_content = tmp.read("billing/pricing.md");
    assert!(
        pricing_content
            .contains("## Enterprise Tier\n\nSee [Enterprise Pricing](enterprise.md).\n")
    );
    assert!(pricing_content.contains("## Community Tier"));
}

#[test]
fn cli_merge_consolidates_concepts() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "billing/discounts.md",
        "---\ntype: Concept\ntitle: Discounts\n---\n\n# Discounts\n\nCoupon codes.\n",
    );
    tmp.write(
        "billing/pricing.md",
        "---\ntype: Concept\ntitle: Pricing\n---\n\n# Pricing\n\nPricing details.\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Concept\ntitle: Overview\n---\n\n# Overview\n\nCheck [Discounts](billing/discounts.md).\n",
    );

    let output = okf()
        .args([
            "merge-concepts",
            "billing/discounts",
            "billing/pricing",
            "--bundle",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(val["source"], "billing/discounts");
    assert_eq!(val["target"], "billing/pricing");
    assert_eq!(val["rewritten_links_count"], 1);

    assert!(!tmp.path().join("billing/discounts.md").exists());

    let pricing_content = tmp.read("billing/pricing.md");
    assert!(pricing_content.contains("## Discounts"));
    assert!(pricing_content.contains("Coupon codes."));

    let overview_content = tmp.read("overview.md");
    assert!(overview_content.contains("[Discounts](billing/pricing.md)"));
}

#[test]
fn cli_mv_renames_section_and_rewrites_anchors() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "billing/pricing.md",
        "---\ntype: Concept\ntitle: Pricing\n---\n\n# Pricing\n\nSee [Internal Tiers](#pricing-tiers).\n\n## Pricing Tiers\n\nTier details.\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Concept\ntitle: Overview\n---\n\n# Overview\n\nCheck [Pricing Plans](billing/pricing.md#pricing-tiers).\n",
    );

    let output = okf()
        .args([
            "mv",
            "billing/pricing#pricing-tiers",
            "billing/pricing#subscription-plans",
            "--bundle",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(val["status"], "ok");
    assert_eq!(val["kind"], "rename_section");
    assert_eq!(val["internal_links_updated"], 1);
    assert_eq!(val["external_links_updated"], 1);

    let pricing_content = tmp.read("billing/pricing.md");
    assert!(
        pricing_content.contains("## subscription-plans")
            || pricing_content.contains("## Subscription Plans")
            || pricing_content.contains("subscription-plans")
    );
    assert!(pricing_content.contains("[Internal Tiers](#subscription-plans)"));

    let overview_content = tmp.read("overview.md");
    assert!(overview_content.contains("[Pricing Plans](billing/pricing.md#subscription-plans)"));
}

#[test]
fn cli_refactor_exit_codes_missing_bundle_and_data_error() {
    // 1. Missing bundle path -> EX_NOINPUT (66)
    let output_no_input = okf()
        .args([
            "mv",
            "auth",
            "security",
            "--bundle",
            "/non/existent/bundle/dir",
        ])
        .output()
        .unwrap();
    assert_eq!(output_no_input.status.code(), Some(66));

    // 2. Concept not found -> EX_DATAERR (65)
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");

    let output_data_err = okf()
        .args([
            "mv",
            "ghost_concept",
            "target",
            "--bundle",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output_data_err.status.code(), Some(65));

    // 3. Section not found on split -> EX_DATAERR (65)
    tmp.write("concept.md", "---\ntype: Concept\n---\n\n# Concept\n");
    let output_sec_err = okf()
        .args([
            "split",
            "concept",
            "new_concept",
            "--section",
            "NonExistentSection",
            "--bundle",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output_sec_err.status.code(), Some(65));
}

#[test]
fn cli_refactor_author_attribution_and_path_resolution() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "module/service.md",
        "---\ntype: Concept\ntitle: Service\n---\n\n# Service\n\n## Implementation\n\n```python\n# ## Implementation\npass\n```\n\nDetails.\n",
    );

    // 1. Rename with ./ relative paths and --author
    let output = okf()
        .args([
            "mv",
            "./module/service.md",
            "./module/daemon.md",
            "--bundle",
            tmp.path().to_str().unwrap(),
            "--author",
            "human:alice",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!tmp.path().join("module/service.md").exists());
    assert!(tmp.path().join("module/daemon.md").exists());

    let log_content = tmp.read("log.md");
    assert!(log_content.contains("(by human:alice)"));

    // 2. Split section with code block immunity and --author
    let split_out = okf()
        .args([
            "split",
            "module/daemon",
            "module/impl",
            "--section",
            "Implementation",
            "--bundle",
            tmp.path().to_str().unwrap(),
            "--author",
            "human:bob",
        ])
        .output()
        .unwrap();

    assert!(split_out.status.success());
    assert!(tmp.path().join("module/impl.md").exists());

    let log_after_split = tmp.read("log.md");
    assert!(log_after_split.contains("(by human:bob)"));

    let daemon_content = tmp.read("module/daemon.md");
    assert!(daemon_content.contains("[Implementation](impl.md)"));
}

#[test]
fn cli_merge_force_flag_accepted() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "a.md",
        "---\ntype: Concept\ntitle: A\n---\n\n# A\n\nContent A.\n",
    );
    tmp.write(
        "b.md",
        "---\ntype: Concept\ntitle: B\n---\n\n# B\n\nContent B.\n",
    );

    let output = okf()
        .args([
            "merge",
            "a",
            "b",
            "--force",
            "--bundle",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!tmp.path().join("a.md").exists());
    assert!(tmp.path().join("b.md").exists());
}

#[test]
fn cli_refactor_infers_bundle_from_source_file_path() {
    let tmp = TempDir::new();
    let bundle_dir = tmp.path().join("my_bundle");
    std::fs::create_dir_all(&bundle_dir).unwrap();

    std::fs::write(
        bundle_dir.join("index.md"),
        "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n",
    )
    .unwrap();
    std::fs::write(
        bundle_dir.join("source.md"),
        "---\ntype: Concept\ntitle: Source\n---\n\n# Source\n",
    )
    .unwrap();

    let outside_dir = tmp.path().join("outside");
    std::fs::create_dir_all(&outside_dir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_okf"))
        .args([
            "mv",
            bundle_dir.join("source.md").to_str().unwrap(),
            "target",
        ])
        .current_dir(&outside_dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!bundle_dir.join("source.md").exists());
    assert!(bundle_dir.join("target.md").exists());
}
