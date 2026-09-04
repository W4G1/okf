//! Integration tests: snapshot pipeline, reducer behavior, worker
//! round-trips against tempdir bundles, and `TestBackend` rendering.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use okf_core::{ConceptId, Date};
use okf_studio::app::{App, Command, Msg, Overlay, PreviewReport, RefactorOp, TreeSel};
use okf_studio::snapshot::Snapshot;
use okf_studio::{StudioOptions, Tab};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::Duration;

/// A scratch bundle on disk, cleaned up on drop.
struct TestBundle {
    root: PathBuf,
}

impl Drop for TestBundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Builds the fixture bundle the tests share: a mix of tiers, staleness,
/// a computation contract, a broken link, and log history.
fn fixture() -> TestBundle {
    let root = std::env::temp_dir().join(format!(
        "okf-studio-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    write(
        &root,
        "index.md",
        "---\nokf_version: \"0.2\"\n---\n\n# Concept\n\n* [Travel](policies/travel_expenses.md) - travel policy\n",
    );
    write(
        &root,
        "log.md",
        "# Update Log\n\n## 2026-08-20\n* **Update**: Adjusted `policies/travel_expenses`.\n\n## 2026-08-01\n* **Creation**: Established the bundle.\n",
    );
    write(
        &root,
        "policies/travel_expenses.md",
        "---\ntype: Policy\ntitle: Travel and expense policy\ndescription: Reimbursement for travel.\nstatus: stable\ngenerated: { by: reference_agent/gemini-3.7-flash, at: 2026-06-20T22:53:05Z }\nverified: { by: human:sarah_hr, at: 2026-06-25T09:00:00Z }\nstale_after: 2026-12-31T00:00:00Z\ntags: [hr, travel]\nsources:\n  - id: mileage-guide\n    resource: https://example.com/mileage\n    title: Standard mileage guidelines\n---\n\n# Travel and expense policy\n\nEmployees are reimbursed at approved rates.[^mileage-guide]\n\nTotal reimbursement uses the [Mileage calculator](../computations/mileage_calc.md)\nand relates to [Paid time off](paid_time_off.md).\n\n## Reimbursement rates\n\nStandard rates apply.\n\n[^mileage-guide]: Standard mileage reimbursement guidelines\n",
    );
    write(
        &root,
        "policies/paid_time_off.md",
        "---\ntype: Policy\ntitle: Paid time off\nverified:\n  - { by: process:hr-nightly, at: 2026-07-01T02:00:00Z }\n---\n\n# Paid time off\n\nSee [Travel](travel_expenses.md).\n",
    );
    write(
        &root,
        "policies/remote_work.md",
        "---\ntype: Policy\ntitle: Remote work\nstatus: draft\nstale_after: 2026-08-13T00:00:00Z\n---\n\n# Remote work\n\nLinks to a [missing page](../legacy/old_faq.md).\n",
    );
    write(
        &root,
        "computations/mileage_calc.md",
        "---\ntype: Attested Computation\ntitle: Mileage calculator\nruntime: python\nparameters:\n  - { name: miles, type: number, required: true }\n  - { name: rate_per_mile, type: number }\nexecutor:\n  resource: references/skills/submit_expense.md\n  receipt: [report_id, calculated_amount, status]\nattester:\n  resource: references/attesters/verify_rate.py\n---\n\n# Computation\n\n```python\ndef calculate(miles: float, rate: float = 0.67) -> float:\n    return round(miles * rate, 2)\n```\n",
    );
    write(
        &root,
        "references/skills/submit_expense.md",
        "---\ntype: Skill\n---\n\n# Submit\n",
    );
    write(
        &root,
        "references/attesters/verify_rate.py",
        "def verify(receipt):\n    return True\n",
    );
    TestBundle { root }
}

const TODAY: Date = Date {
    year: 2026,
    month: 8,
    day: 25,
};

fn snapshot(bundle: &TestBundle) -> Arc<Snapshot> {
    Arc::new(Snapshot::build(&bundle.root, Some(TODAY), 1).unwrap())
}

fn app_with(bundle: &TestBundle) -> App {
    let mut app = App::new(&StudioOptions {
        root: bundle.root.clone(),
        today: Some(TODAY),
        no_watch: true,
        initial_tab: None,
        author: Some("human:tester".to_string()),
    });
    app.pending_commands.clear();
    app.update(Msg::SnapshotReady(snapshot(bundle)));
    app
}

fn key(app: &mut App, code: KeyCode) {
    app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn ch(app: &mut App, c: char) {
    key(app, KeyCode::Char(c));
}

// ---------------------------------------------------------------------------
// Snapshot pipeline.
// ---------------------------------------------------------------------------

#[test]
fn snapshot_derives_the_world_model() {
    let bundle = fixture();
    let snap = snapshot(&bundle);

    assert_eq!(snap.stats.concepts, 5);
    assert_eq!(snap.stats.stale, 1); // remote_work passed its stale_after
    assert_eq!(snap.stats.broken_links, 1);
    assert_eq!(snap.contracts.len(), 1);
    assert!(
        snap.contracts[0].healthy(),
        "{:?}",
        snap.contracts[0].issues
    );

    let travel = ConceptId::parse("policies/travel_expenses").unwrap();
    let meta = snap.meta(&travel).unwrap();
    assert_eq!(meta.tier, okf_core::TrustTier::HumanReviewed);
    assert_eq!(meta.out_degree, 2);
    assert!(!meta.stale);
    assert!(
        meta.headings
            .iter()
            .any(|(_, t)| t == "Reimbursement rates")
    );

    // The attention queue leads with the stale draft.
    assert!(!snap.attention.is_empty());
    assert_eq!(snap.attention[0].id.to_string(), "policies/remote_work");

    // Search: segment initials find the concept; filters compose.
    let hits = snap.search.search("pte", 10);
    assert_eq!(hits[0].id, travel);
    let stale_hits = snap.search.search("is:stale", 10);
    assert_eq!(stale_hits.len(), 1);
    assert_eq!(stale_hits[0].id.to_string(), "policies/remote_work");

    // Log views merged.
    assert_eq!(snap.log_timeline.iter().map(|(_, c)| c).sum::<usize>(), 2);
    assert_eq!(snap.log_days.first().unwrap().0, "2026-08-20");

    // Graph model: 5 concepts + 1 phantom (broken target) + 1 source node.
    assert_eq!(snap.graph.nodes.len(), 7);
}

// ---------------------------------------------------------------------------
// Markdown renderer.
// ---------------------------------------------------------------------------

#[test]
fn renderer_maps_links_headings_and_footnotes() {
    let theme = okf_studio::theme::Theme::with_color(false);
    let body = "# Title\n\nSee [other](./other.md) and a note.[^src]\n\n```python\nx = 1\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n[^src]: The source\n";
    let doc = okf_studio::markdown::render_document(body, 60, &theme, None);
    assert_eq!(doc.headings.len(), 1);
    assert_eq!(doc.links.len(), 2); // one link + one footnote ref
    assert!(doc.footnote_defs.contains_key("src"));
    let text: String = doc
        .lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                + "\n"
        })
        .collect();
    assert!(text.contains("→other"));
    // The verdict badge needs the Python parser, which sits behind the
    // `python` feature; without it the block renders with no badge at all.
    if cfg!(feature = "python") {
        assert!(text.contains("syntax ✔"), "code fence verdict shown");
    } else {
        assert!(
            !text.contains("syntax"),
            "no verdict without a parser: {text}"
        );
    }
    assert!(text.contains("│ 1 │ 2 │"), "table box-drawn: {text}");
}

#[test]
fn renderer_survives_adversarial_input() {
    let theme = okf_studio::theme::Theme::with_color(false);
    for body in [
        "```\nunclosed fence",
        "[unclosed link](nowhere",
        "**unclosed bold and `code",
        "| lonely table cell",
        "日本語のテキストが折り返される長い行です。日本語のテキストが折り返される長い行です。",
        "",
    ] {
        let doc = okf_studio::markdown::render_document(body, 20, &theme, None);
        let _ = doc.lines.len(); // must not panic
    }
}

// ---------------------------------------------------------------------------
// Reducer.
// ---------------------------------------------------------------------------

#[test]
fn keys_drive_tabs_palette_and_quit() {
    let bundle = fixture();
    let mut app = app_with(&bundle);
    assert_eq!(app.tab, Tab::Explorer);

    ch(&mut app, '2');
    assert_eq!(app.tab, Tab::Graph);
    ch(&mut app, '3');
    assert_eq!(app.tab, Tab::Trust);
    ch(&mut app, '1');
    assert_eq!(app.tab, Tab::Explorer);

    // Omnisearch: type a query, Enter jumps to the top hit.
    ch(&mut app, '/');
    assert!(matches!(app.overlays.last(), Some(Overlay::Palette(_))));
    for c in "pto".chars() {
        ch(&mut app, c);
    }
    key(&mut app, KeyCode::Enter);
    assert!(app.overlays.is_empty());
    assert_eq!(
        app.explorer.selected,
        Some(TreeSel::Concept(
            ConceptId::parse("policies/paid_time_off").unwrap()
        ))
    );

    // Esc closes overlays; q quits at the root.
    ch(&mut app, '?');
    assert!(matches!(app.overlays.last(), Some(Overlay::Help(_))));
    key(&mut app, KeyCode::Esc);
    assert!(app.overlays.is_empty());
    ch(&mut app, 'q');
    assert!(app.should_quit);
}

#[test]
fn selection_survives_snapshot_swaps() {
    let bundle = fixture();
    let mut app = app_with(&bundle);
    let pto = ConceptId::parse("policies/paid_time_off").unwrap();
    app.open_concept(&pto);
    assert_eq!(app.explorer.selected, Some(TreeSel::Concept(pto.clone())));

    // Same concept still present: selection unchanged.
    app.update(Msg::SnapshotReady(snapshot(&bundle)));
    assert_eq!(app.explorer.selected, Some(TreeSel::Concept(pto.clone())));

    // Concept removed underneath us: selection falls back to the first.
    std::fs::remove_file(bundle.root.join("policies/paid_time_off.md")).unwrap();
    app.update(Msg::SnapshotReady(snapshot(&bundle)));
    assert_ne!(app.explorer.selected, Some(TreeSel::Concept(pto)));
    assert!(app.explorer.selected.is_some());
}

#[test]
fn remove_verb_debounces_into_a_preview_command() {
    let bundle = fixture();
    let mut app = app_with(&bundle);
    let pto = ConceptId::parse("policies/paid_time_off").unwrap();
    app.open_concept(&pto);
    app.pending_commands.clear();

    key(&mut app, KeyCode::Delete);
    assert!(matches!(app.overlays.last(), Some(Overlay::Refactor(_))));

    // The dry-run request is debounced onto the next tick.
    app.update(Msg::Tick);
    let preview = app
        .pending_commands
        .iter()
        .find(|c| matches!(c, Command::Preview { .. }));
    assert!(preview.is_some(), "{:?}", app.pending_commands);
}

#[test]
fn verify_confirm_emits_stamp_command() {
    let bundle = fixture();
    let mut app = app_with(&bundle);
    let travel = ConceptId::parse("policies/travel_expenses").unwrap();
    app.open_concept(&travel);
    app.pending_commands.clear();

    ch(&mut app, 'v');
    assert!(matches!(app.overlays.last(), Some(Overlay::Confirm(_))));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.pending_commands.first(),
        Some(Command::StampVerification(id)) if *id == travel
    ));
}

// ---------------------------------------------------------------------------
// Worker round-trips.
// ---------------------------------------------------------------------------

fn recv_until<F: Fn(&Msg) -> bool>(rx: &std::sync::mpsc::Receiver<Msg>, accept: F) -> Msg {
    loop {
        let msg = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("worker reply");
        if accept(&msg) {
            return msg;
        }
    }
}

#[test]
fn worker_reloads_stamps_and_refactors() {
    let bundle = fixture();
    let (tx, rx) = channel();
    let worker = okf_studio::worker::spawn(
        okf_studio::worker::WorkerConfig {
            root: bundle.root.clone(),
            today: Some(TODAY),
            author: "human:tester".to_string(),
        },
        tx,
    );

    worker.send(Command::Reload).unwrap();
    let Msg::SnapshotReady(snap) = recv_until(&rx, |m| matches!(m, Msg::SnapshotReady(_))) else {
        unreachable!()
    };
    assert_eq!(snap.generation, 1);

    // Verification stamp: unverified concept becomes human-reviewed on disk,
    // and the log records it.
    let remote = ConceptId::parse("policies/remote_work").unwrap();
    worker
        .send(Command::StampVerification(remote.clone()))
        .unwrap();
    let Msg::Applied(result) = recv_until(&rx, |m| matches!(m, Msg::Applied(_))) else {
        unreachable!()
    };
    assert!(result.is_ok(), "{result:?}");
    let Msg::SnapshotReady(snap) = recv_until(&rx, |m| matches!(m, Msg::SnapshotReady(_))) else {
        unreachable!()
    };
    assert_eq!(
        snap.meta(&remote).unwrap().tier,
        okf_core::TrustTier::HumanReviewed
    );
    let log_text = std::fs::read_to_string(bundle.root.join("log.md")).unwrap();
    assert!(log_text.contains("Verified concept `policies/remote_work`"));

    // Move preview then apply: dry-run touches nothing, apply rewrites links.
    let source = ConceptId::parse("policies/travel_expenses").unwrap();
    let target = ConceptId::parse("policies/travel").unwrap();
    let op = RefactorOp::Move {
        source: source.clone(),
        target: target.clone(),
        force: false,
    };
    worker
        .send(Command::Preview {
            request: 7,
            op: op.clone(),
        })
        .unwrap();
    let Msg::PreviewReady(7, Ok(PreviewReport::Move(report))) =
        recv_until(&rx, |m| matches!(m, Msg::PreviewReady(..)))
    else {
        panic!("expected a move preview")
    };
    assert!(report.dry_run);
    assert!(report.rewritten_incoming_links >= 1);
    assert!(bundle.root.join("policies/travel_expenses.md").exists());

    worker.send(Command::Apply(op)).unwrap();
    let Msg::Applied(result) = recv_until(&rx, |m| matches!(m, Msg::Applied(_))) else {
        unreachable!()
    };
    assert!(result.is_ok(), "{result:?}");
    let Msg::SnapshotReady(snap) = recv_until(&rx, |m| matches!(m, Msg::SnapshotReady(_))) else {
        unreachable!()
    };
    assert!(!bundle.root.join("policies/travel_expenses.md").exists());
    assert!(snap.bundle.contains(&target));
    let pto_text = std::fs::read_to_string(bundle.root.join("policies/paid_time_off.md")).unwrap();
    assert!(pto_text.contains("travel.md"), "{pto_text}");

    worker.send(Command::Shutdown).unwrap();
}

#[test]
fn worker_extends_stale_after() {
    let bundle = fixture();
    let (tx, rx) = channel();
    let worker = okf_studio::worker::spawn(
        okf_studio::worker::WorkerConfig {
            root: bundle.root.clone(),
            today: Some(TODAY),
            author: "human:tester".to_string(),
        },
        tx,
    );
    worker.send(Command::Reload).unwrap();
    let _ = recv_until(&rx, |m| matches!(m, Msg::SnapshotReady(_)));

    let remote = ConceptId::parse("policies/remote_work").unwrap();
    let new_date = Date {
        year: 2026,
        month: 12,
        day: 31,
    };
    worker
        .send(Command::SetStaleAfter(remote.clone(), new_date))
        .unwrap();
    let _ = recv_until(&rx, |m| matches!(m, Msg::Applied(_)));
    let Msg::SnapshotReady(snap) = recv_until(&rx, |m| matches!(m, Msg::SnapshotReady(_))) else {
        unreachable!()
    };
    let meta = snap.meta(&remote).unwrap();
    assert!(!meta.stale);
    assert_eq!(
        meta.stale_after.as_ref().unwrap().raw,
        "2026-12-31T00:00:00Z"
    );
    worker.send(Command::Shutdown).unwrap();
}

// ---------------------------------------------------------------------------
// TestBackend rendering.
// ---------------------------------------------------------------------------

fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = *buffer.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn draw(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| okf_studio::ui::shell::draw(frame, app))
        .unwrap();
    buffer_text(&terminal)
}

#[test]
fn every_workspace_renders_at_both_sizes() {
    let bundle = fixture();
    let mut app = app_with(&bundle);

    for (w, h) in [(80, 24), (120, 40)] {
        app.tab = Tab::Explorer;
        let text = draw(&app, w, h);
        assert!(text.contains("okf studio"));
        assert!(text.contains("Explorer"));
        assert!(text.contains("travel_exp"), "{text}"); // may clip at 80 cols
        assert!(text.contains("conformant"));
        assert!(text.contains("5 concepts"));

        app.tab = Tab::Graph;
        let text = draw(&app, w, h);
        assert!(text.contains("Graph"));
        assert!(text.contains("layout: force"));
        assert!(text.contains("color: trust"));

        app.tab = Tab::Trust;
        let text = draw(&app, w, h);
        assert!(text.contains("Mission Control"));
        assert!(text.contains("ATTENTION QUEUE"));
        assert!(text.contains("remote_work"));
        assert!(text.contains("ACTORS"));

        app.tab = Tab::Computations;
        let text = draw(&app, w, h);
        assert!(text.contains("Computations"));
        assert!(text.contains("mileage_calc"));
        assert!(text.contains("PLAYGROUND"));
        assert!(text.contains("call sketch"));
    }
}

#[test]
fn overlays_render() {
    let bundle = fixture();
    let mut app = app_with(&bundle);

    ch(&mut app, '/');
    for c in "trav".chars() {
        ch(&mut app, c);
    }
    let text = draw(&app, 100, 30);
    assert!(text.contains("trav"));
    assert!(text.contains("travel"));

    key(&mut app, KeyCode::Esc);
    ch(&mut app, '!');
    let text = draw(&app, 100, 30);
    assert!(text.contains("Diagnostics"));

    key(&mut app, KeyCode::Esc);
    let travel = ConceptId::parse("policies/travel_expenses").unwrap();
    app.open_concept(&travel);
    key(&mut app, KeyCode::F(2));
    let text = draw(&app, 100, 30);
    assert!(text.contains("Move policies/travel_expenses"));
    assert!(text.contains("target id"));
}
