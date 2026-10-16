//! Process-level tests for the CLI exit-code ladder and the resolution→report
//! diagnostics splice. These drive the real `rcd` binary (no new deps) to pin
//! the wiring that in-process unit tests cannot reach: `process::exit` codes and
//! the `report.diagnostics.splice(0..0, ..)` merge.

use std::fs;
use std::process::Command;

const VALID: &str = "<ModMetaData><packageId>test.good</packageId></ModMetaData>";
const BROKEN: &str = "<ModMetaData><unclosed>";

fn write_about(mod_dir: &std::path::Path, xml: &str) {
    let about = mod_dir.join("About");
    fs::create_dir_all(&about).unwrap();
    fs::write(about.join("About.xml"), xml).unwrap();
}

fn rcd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rcd"))
}

fn stdout_string(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_string(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// (a) Every mod fails to resolve → exit 2, but JSON mode still emits a full,
/// parseable report carrying the diagnostics (the I-1 fix).
#[test]
fn json_total_failure_exits_2_with_report_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    write_about(&dir.path().join("BrokenMod"), BROKEN);

    let out = rcd()
        .args([
            "--format",
            "json",
            "scan",
            "--dir",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2; stdout={} stderr={}",
        stdout_string(&out),
        stderr_string(&out)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be valid JSON on total failure");
    assert!(
        json["metadata"]["mods_scanned"]
            .as_array()
            .expect("mods_scanned array")
            .is_empty(),
        "no mods resolved: {}",
        stdout_string(&out)
    );
    assert!(
        json["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .any(|d| d["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Failed to parse mod")),
        "resolution diagnostic must be present in JSON: {}",
        stdout_string(&out)
    );
}

/// (b) A clean single-mod scan → exit 0.
#[test]
fn clean_scan_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    write_about(&dir.path().join("GoodMod"), VALID);

    let out = rcd()
        .args([
            "--format",
            "json",
            "scan",
            "--dir",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0; stdout={} stderr={}",
        stdout_string(&out),
        stderr_string(&out)
    );

    // Pin the happy-path JSON shape too: one scanned mod, zero diagnostics.
    // Without this, a stray diagnostic leak on the clean path stays green.
    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("clean scan must emit valid JSON");
    assert_eq!(
        report["metadata"]["mods_scanned"].as_array().unwrap().len(),
        1
    );
    assert!(report["diagnostics"].as_array().unwrap().is_empty());
}

/// (e) Qwen review I-2: an empty load order plus one installed-but-inactive
/// mod with a malformed LoadFolders.xml used to publish that candidate's
/// diagnostic and therefore exit 2 ("total resolution failure") for a scan
/// that resolved exactly what was asked for.
#[test]
fn empty_active_config_with_inactive_malformed_loadfolders_exits_0() {
    let root = tempfile::tempdir().unwrap();
    let inactive = root.path().join("Mods/InactiveMod");
    write_about(&inactive, VALID);
    fs::write(
        inactive.join("LoadFolders.xml"),
        "<loadFolders><v1.5><li>broken</loadFolders>",
    )
    .unwrap();
    let config = root.path().join("ModsConfig.xml");
    fs::write(
        &config,
        "<ModsConfigData><version>1.5</version><activeMods></activeMods></ModsConfigData>",
    )
    .unwrap();

    let out = rcd()
        .args([
            "--format",
            "json",
            "scan",
            "--config",
            config.to_str().unwrap(),
            "--rimworld-path",
            root.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|error| panic!("stdout must be JSON ({error}): {}", stdout_string(&out)));
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0; stdout={} stderr={}",
        stdout_string(&out),
        stderr_string(&out)
    );
    assert!(
        json["metadata"]["mods_scanned"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        json["diagnostics"].as_array().unwrap().is_empty(),
        "no diagnostic may reference an unscanned mod: {}",
        stdout_string(&out)
    );
}

/// (c) A valid mod plus an explicitly-named broken path → the good mod is
/// scanned (so not exit 2), there are no conflicts (so not exit 1), but an
/// Error-severity diagnostic exists → exit 3.
#[test]
fn error_diagnostic_without_conflicts_exits_3() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("BrokenMod");
    let good = dir.path().join("GoodMod");
    write_about(&broken, BROKEN);
    write_about(&good, VALID);

    let out = rcd()
        .args([
            "--format",
            "json",
            "scan",
            "--mods",
            broken.to_str().unwrap(),
            good.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(3),
        "expected exit 3; stdout={} stderr={}",
        stdout_string(&out),
        stderr_string(&out)
    );

    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        !json["metadata"]["mods_scanned"]
            .as_array()
            .expect("mods_scanned array")
            .is_empty(),
        "the good mod must still be scanned: {}",
        stdout_string(&out)
    );
    assert!(
        json["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .any(|d| d["severity"] == "error"
                && d["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("Failed to parse mod")),
        "an Error diagnostic must be present: {}",
        stdout_string(&out)
    );
}
