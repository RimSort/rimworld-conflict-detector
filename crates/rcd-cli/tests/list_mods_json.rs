use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn write_mod(root: &std::path::Path, name: &str, package_id: &str, about: &str) {
    let about_dir = root.join(name).join("About");
    fs::create_dir_all(&about_dir).unwrap();
    fs::write(
        about_dir.join("About.xml"),
        format!(
            "<ModMetaData><packageId>{package_id}</packageId><name>{name}</name></ModMetaData>"
        ),
    )
    .unwrap();
    if !about.is_empty() {
        fs::write(about_dir.join("About.xml"), about).unwrap();
    }
}

fn run_list_mods(dir: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rcd"))
        .args(["list-mods", "--dir"])
        .arg(dir)
        .args(["--format", "json"])
        .output()
        .unwrap()
}

#[test]
fn human_list_keeps_mod_text_and_resolution_diagnostics_on_stderr() {
    let root = tempdir().unwrap();
    write_mod(root.path(), "Good", "example.good", "");
    let broken_about = root.path().join("Broken/About/About.xml");
    fs::create_dir_all(broken_about.parent().unwrap()).unwrap();
    fs::write(&broken_about, "<ModMetaData><packageId>").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rcd"))
        .args(["list-mods", "--dir"])
        .arg(root.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("Good (example.good)"), "{stdout}");
    assert!(stderr.contains("Failed to parse mod at"), "{stderr}");
}

#[test]
fn json_success_outputs_mods_and_empty_diagnostics_envelope() {
    let root = tempdir().unwrap();
    write_mod(root.path(), "Good", "example.good", "");

    let output = run_list_mods(root.path());

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["mods"].as_array().unwrap().len(), 1);
    assert_eq!(json["mods"][0]["package_id"], "example.good");
    assert_eq!(json["diagnostics"], serde_json::json!([]));
}

#[test]
fn json_empty_failure_outputs_diagnostic_envelope_without_prose() {
    let root = tempdir().unwrap();
    let missing = root.path().join("missing");

    let output = run_list_mods(&missing);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stderr.is_empty(),
        "JSON mode must not write non-JSON prose to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["mods"], serde_json::json!([]));
    let diagnostics = json["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );
}

#[test]
fn json_partial_resolution_outputs_mods_and_warning_envelope() {
    let root = tempdir().unwrap();
    write_mod(root.path(), "Good", "example.good", "");
    let broken_about = root.path().join("Broken/About/About.xml");
    fs::create_dir_all(broken_about.parent().unwrap()).unwrap();
    fs::write(&broken_about, "<ModMetaData><packageId>").unwrap();

    let output = run_list_mods(root.path());

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["mods"].as_array().unwrap().len(), 1);
    let diagnostics = json["diagnostics"].as_array().unwrap();
    assert!(!diagnostics.is_empty());
    assert!(diagnostics.iter().any(|d| d["severity"] == "warning"));
}
