use rcd_core::folder::discover_mods;
use rcd_core::report::ReportMetadata;
use rcd_core::*;

fn fixtures_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/mods")
}

fn run_analysis(mods: &[Mod], order: Option<&LoadOrder>) -> Report {
    let mut engine = Engine::new();
    engine.add_detector(Box::new(rcd_xml::XmlDetector));
    engine.add_detector(Box::new(rcd_harmony::HarmonyDetector));

    let metadata = ReportMetadata {
        timestamp: "test".into(),
        game_version: Some("1.5".into()),
        order_aware: order.is_some(),
        mods_scanned: mods.iter().map(ModSummary::from).collect(),
    };

    engine.analyze(mods, order, metadata, None)
}

#[test]
fn detect_direct_replacement_conflict() {
    let mods = discover_mods(&[fixtures_path()], Some("1.5"));
    let report = run_analysis(&mods, None);

    // Should find ThingDef:Steel replacement conflict
    let errors: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| c.severity() == Severity::Error)
        .collect();
    assert!(!errors.is_empty(), "Should detect replacement conflicts");
}

#[test]
fn report_has_correct_schema_version() {
    let mods = discover_mods(&[fixtures_path()], Some("1.5"));
    let report = run_analysis(&mods, None);
    assert_eq!(report.schema_version, 1);
}

#[test]
fn report_json_is_valid() {
    let mods = discover_mods(&[fixtures_path()], Some("1.5"));
    let report = run_analysis(&mods, None);
    let json = serde_json::to_string_pretty(&report).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(!parsed["conflicts"].as_array().unwrap().is_empty());
}

#[test]
fn order_aware_reports_winner() {
    let mods = discover_mods(&[fixtures_path()], Some("1.5"));
    let order = LoadOrder {
        package_ids: mods.iter().map(|m| m.package_id.clone()).collect(),
    };
    let report = run_analysis(&mods, Some(&order));

    // With load order, some conflicts should have a winner
    let has_winner = report.conflicts.iter().any(|c| {
        if let Conflict::Def(dc) = c {
            dc.winner.is_some()
        } else {
            false
        }
    });
    assert!(has_winner, "Order-aware analysis should identify winners");
}

#[test]
fn summary_counts_match_conflicts() {
    let mods = discover_mods(&[fixtures_path()], Some("1.5"));
    let report = run_analysis(&mods, None);

    let actual_errors = report
        .conflicts
        .iter()
        .filter(|c| c.severity() == Severity::Error)
        .count() as u32;
    assert_eq!(report.summary.conflicts_by_severity.error, actual_errors);
}
