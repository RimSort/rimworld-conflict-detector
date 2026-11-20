// RimWorld Conflict Detector - FFI surface

use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// `phase` points to a string valid only during this synchronous callback invocation.
pub type ProgressCallback = extern "C" fn(current: u32, total: u32, phase: *const c_char);

#[derive(Debug, Deserialize)]
struct RcdConfig {
    #[serde(default)]
    mod_paths: Vec<std::path::PathBuf>,
    #[serde(default, deserialize_with = "deserialize_game_version")]
    game_version: Option<String>,
    #[serde(default)]
    ordered: bool,
    #[serde(flatten)]
    extras: BTreeMap<String, serde_json::Value>,
}

fn deserialize_game_version<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

/// Analyze mods for conflicts.
///
/// Takes a JSON config string specifying mod paths and options.
/// Returns a JSON report string. Always returns valid JSON.
/// The caller must free the returned string with `rcd_free`.
///
/// # Safety
///
/// `config_json` must be a valid, null-terminated C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rcd_analyze(
    config_json: *const c_char,
    progress: Option<ProgressCallback>,
) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        let config_str = {
            if config_json.is_null() {
                return error_json("config_json is null");
            }
            match unsafe { CStr::from_ptr(config_json) }.to_str() {
                Ok(s) => s.to_owned(),
                Err(e) => return error_json(&format!("Invalid UTF-8 in config: {}", e)),
            }
        };

        // Parse config strictly: known fields with invalid types are errors.
        let config: RcdConfig = match serde_json::from_str(&config_str) {
            Ok(config) => config,
            Err(e) => return error_json(&format!("Invalid config JSON: {}", e)),
        };
        let game_version = config.game_version.as_deref();
        let ordered = config.ordered;

        // Parse all metadata first so every supplied mod is active when conditions resolve.
        let mut mods = Vec::new();
        let mut pre_diagnostics: Vec<rcd_core::Diagnostic> = Vec::new();
        if !config.extras.is_empty() {
            let keys = config.extras.keys().cloned().collect::<Vec<_>>().join(", ");
            pre_diagnostics.push(rcd_core::Diagnostic {
                affected_mod: None,
                severity: rcd_core::Severity::Warning,
                message: format!("Unknown config fields ignored: {keys}"),
                source: rcd_core::DiagnosticSource::MetadataResolution,
                analysis_incomplete: true,
                code: Some("unknown_config_fields".into()),
                params: [("fields".into(), keys)].into_iter().collect(),
            });
        }
        if let Some(version) = game_version.filter(|v| !v.is_empty())
            && rcd_core::load_folders::normalize_game_version(version).is_none()
        {
            pre_diagnostics.push(rcd_core::Diagnostic {
                affected_mod: None,
                severity: rcd_core::Severity::Warning,
                message: format!(
                    "Invalid game version {version:?}; falling back to default folder selection"
                ),
                source: rcd_core::DiagnosticSource::FolderResolution,
                analysis_incomplete: true,
                code: Some("invalid_game_version".into()),
                params: [
                    ("version".into(), version.to_owned()),
                    ("version_debug".into(), format!("{version:?}")),
                ]
                .into_iter()
                .collect(),
            });
        }
        let mut package_paths: BTreeMap<String, std::path::PathBuf> = BTreeMap::new();
        let mut duplicate = None;
        for path in &config.mod_paths {
            match rcd_core::about_xml::parse_about_xml(path) {
                Ok(m) => {
                    if let Some(first) = package_paths.get(&m.package_id) {
                        if duplicate.is_none() {
                            duplicate = Some(format!(
                                "Duplicate package ID {}: {} and {}",
                                m.package_id,
                                std::path::Path::display(first),
                                path.display()
                            ));
                        }
                    } else {
                        package_paths.insert(m.package_id.clone(), path.clone());
                    }
                    mods.push(m);
                }
                Err(e) => pre_diagnostics.push(rcd_core::diagnostic::about_parse_failure(
                    path,
                    &e,
                    rcd_core::Severity::Error,
                    rcd_core::DiagnosticSource::MetadataResolution,
                )),
            }
        }
        if let Some(error) = duplicate {
            let json =
                serde_json::json!({"error": error, "diagnostics": pre_diagnostics}).to_string();
            return CString::new(json)
                .expect("JSON cannot contain a raw NUL")
                .into_raw();
        }
        rcd_core::content_files::mark_base_content(&mut mods);
        let active_mods = mods
            .iter()
            .map(rcd_core::ModSummary::from)
            .collect::<Vec<_>>();
        for m in &mut mods {
            pre_diagnostics.extend(rcd_core::folder::resolve_mod_folders_with_active(
                m,
                game_version,
                &active_mods,
            ));
        }

        let load_order = if ordered {
            Some(rcd_core::LoadOrder::new(
                mods.iter().map(|m| m.package_id.clone()).collect(),
            ))
        } else {
            None
        };

        // Set up progress callback wrapper
        let progress_fn: Option<rcd_core::ProgressFn> = progress.map(|cb| {
            let wrapper: rcd_core::ProgressFn = Box::new(move |current, total, phase| {
                let phase_cstr =
                    CString::new(phase).unwrap_or_else(|_| CString::new("unknown").unwrap());
                cb(current, total, phase_cstr.as_ptr());
            });
            wrapper
        });

        // Run analysis
        let mut engine = rcd_core::Engine::new();
        engine.add_detector(Box::new(rcd_xml::XmlDetector));
        engine.add_detector(Box::new(rcd_harmony::HarmonyDetector));

        let metadata = rcd_core::report::ReportMetadata {
            timestamp: chrono::Utc::now().to_rfc3339(),
            game_version: game_version.map(String::from),
            order_aware: load_order.is_some(),
            mods_scanned: mods.iter().map(rcd_core::ModSummary::from).collect(),
        };

        let mut report = engine.analyze(&mods, load_order.as_ref(), metadata, progress_fn.as_ref());
        // Resolution failures must reach C# consumers, not just the CLI.
        report.diagnostics.splice(0..0, pre_diagnostics);

        // Serialize report to JSON
        match serde_json::to_string(&report) {
            Ok(json) => match CString::new(json) {
                Ok(cstr) => cstr.into_raw(),
                Err(e) => error_json(&format!("Report contains null byte: {}", e)),
            },
            Err(e) => error_json(&format!("Failed to serialize report: {}", e)),
        }
    });

    match result {
        Ok(ptr) => ptr,
        Err(_) => error_json("Internal error: panic during analysis"),
    }
}

/// Free a string previously returned by `rcd_analyze`.
///
/// # Safety
///
/// `ptr` must be a pointer previously returned by `rcd_analyze`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rcd_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(unsafe { CString::from_raw(ptr) });
    }
}

fn error_json(msg: &str) -> *mut c_char {
    let json = serde_json::json!({"error": msg}).to_string();
    CString::new(json)
        .unwrap_or_else(|_| CString::new(r#"{"error":"unknown"}"#).unwrap())
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    #[test]
    fn duplicate_parsed_package_ids_are_rejected() {
        let root = TempDir::new();
        let first = make_mod(&root.path().join("first"), "Example.Mod", "One", None);
        let second = make_mod(&root.path().join("second"), "example.mod", "Two", None);
        let reply =
            analyze_json(serde_json::json!({"mod_paths": [first, second], "ordered": true}));
        assert!(reply["error"].as_str().unwrap().contains("example.mod"));
        assert!(reply.get("conflicts").is_none());
    }

    #[test]
    fn duplicate_and_unreadable_metadata_preserve_paths_and_diagnostics() {
        let root = TempDir::new();
        let first = make_mod(&root.path().join("a"), "Example.Mod", "One", None);
        let second = make_mod(&root.path().join("b"), "example.mod", "Two", None);
        let missing = root.path().join("missing");
        let reply = analyze_json(serde_json::json!({"mod_paths": [&first, &second, &missing]}));
        let error = reply["error"].as_str().unwrap();
        assert!(error.contains(&first.display().to_string()));
        assert!(error.contains(&second.display().to_string()));
        let diagnostics = reply["diagnostics"].as_array().unwrap();
        assert!(diagnostics.iter().any(|d| {
            d["analysis_incomplete"] == true
                && d["message"]
                    .as_str()
                    .unwrap()
                    .contains(&missing.display().to_string())
        }));
    }

    #[test]
    fn invalid_game_version_warns_and_preserves_context() {
        let root = TempDir::new();
        let selected = make_mod(
            root.path(),
            "selected.mod",
            "Selected",
            Some(
                "<loadFolders><v1.5><li>Version</li></v1.5><default><li>Fallback</li></default></loadFolders>",
            ),
        );
        let comparison = make_mod(root.path(), "comparison.mod", "Comparison", None);
        for (path, contents) in [
            (
                selected.join("Fallback/Defs"),
                "<Defs><ThingDef><defName>FallbackThing</defName><label>fallback</label></ThingDef></Defs>",
            ),
            (
                selected.join("Version/Defs"),
                "<Defs><ThingDef><defName>VersionThing</defName><label>version</label></ThingDef></Defs>",
            ),
            (
                comparison.join("Defs"),
                "<Defs><ThingDef><defName>FallbackThing</defName><label>comparison</label></ThingDef><ThingDef><defName>VersionThing</defName><label>comparison</label></ThingDef></Defs>",
            ),
        ] {
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("defs.xml"), contents).unwrap();
        }

        for (version, expected_target, should_warn) in [
            (None, "FallbackThing", false),
            (Some(""), "FallbackThing", false),
            (Some("not-a-version"), "FallbackThing", true),
            (Some("   "), "FallbackThing", true),
            (Some("1.5"), "VersionThing", false),
        ] {
            let mut config = serde_json::json!({"mod_paths": [&selected, &comparison]});
            if let Some(version) = version {
                config["game_version"] = version.into();
            }
            let report = analyze_json(config);
            assert_eq!(report["metadata"]["game_version"].as_str(), version);
            let conflicts = report["conflicts"].as_array().unwrap();
            assert_eq!(conflicts.len(), 1, "{version:?}: {report}");
            assert_eq!(
                conflicts[0]["target"]["def_name"], expected_target,
                "{version:?}: {report}"
            );
            let warnings: Vec<_> = report["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| {
                    d["message"]
                        .as_str()
                        .unwrap()
                        .starts_with("Invalid game version ")
                })
                .collect();
            assert_eq!(
                warnings.len(),
                usize::from(should_warn),
                "{version:?}: {report}"
            );
            if should_warn {
                assert_eq!(warnings[0]["code"], "invalid_game_version");
                assert_eq!(warnings[0]["params"]["version"], version.unwrap());
                assert_eq!(warnings[0]["severity"], "warning");
                assert_eq!(warnings[0]["analysis_incomplete"], true);
                assert!(
                    warnings[0]["message"]
                        .as_str()
                        .unwrap()
                        .contains(version.unwrap())
                );
            }
        }
    }

    #[test]
    fn null_config_returns_error() {
        let result = unsafe { rcd_analyze(std::ptr::null(), None) };
        assert!(!result.is_null());
        let result_str = unsafe { CStr::from_ptr(result).to_str().unwrap().to_owned() };
        unsafe { rcd_free(result) };
        assert!(result_str.contains("error"));
    }

    #[test]
    fn invalid_json_returns_error() {
        let config = CString::new("not json").unwrap();
        let result = unsafe { rcd_analyze(config.as_ptr(), None) };
        assert!(!result.is_null());
        let result_str = unsafe { CStr::from_ptr(result).to_str().unwrap().to_owned() };
        unsafe { rcd_free(result) };
        assert!(result_str.contains("error"));
    }

    #[test]
    fn empty_config_returns_valid_report() {
        let config = CString::new(r#"{"mod_paths": []}"#).unwrap();
        let result = unsafe { rcd_analyze(config.as_ptr(), None) };
        assert!(!result.is_null());
        let result_str = unsafe { CStr::from_ptr(result).to_str().unwrap().to_owned() };
        unsafe { rcd_free(result) };

        // Should parse as valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();
        assert_eq!(parsed["schema_version"], 3);
    }

    #[test]
    fn report_has_valid_timestamp() {
        let config = CString::new(r#"{"mod_paths": []}"#).unwrap();
        let result = unsafe { rcd_analyze(config.as_ptr(), None) };
        assert!(!result.is_null());
        let result_str = unsafe { CStr::from_ptr(result).to_str().unwrap().to_owned() };
        unsafe { rcd_free(result) };

        let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();
        let ts = parsed["metadata"]["timestamp"].as_str().unwrap();
        assert!(!ts.is_empty(), "timestamp should not be empty");
        chrono::DateTime::parse_from_rfc3339(ts).expect("timestamp should be valid RFC 3339");
    }

    fn analyze_json(value: serde_json::Value) -> serde_json::Value {
        let config = CString::new(value.to_string()).unwrap();
        let ptr = unsafe { rcd_analyze(config.as_ptr(), None) };
        assert!(!ptr.is_null());
        let json = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        unsafe { rcd_free(ptr) };
        serde_json::from_str(&json).unwrap()
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "rcd-ffi-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_mod(
        root: &std::path::Path,
        id: &str,
        name: &str,
        folders: Option<&str>,
    ) -> std::path::PathBuf {
        let path = root.join(id);
        std::fs::create_dir_all(path.join("About")).unwrap();
        std::fs::write(
            path.join("About/About.xml"),
            format!("<ModMetaData><packageId>{id}</packageId><name>{name}</name></ModMetaData>"),
        )
        .unwrap();
        if let Some(folders) = folders {
            std::fs::write(path.join("LoadFolders.xml"), folders).unwrap();
        }
        path
    }

    #[test]
    fn ffi_serializes_conflict_severity_and_diagnostic_impact() {
        let root = TempDir::new();
        let first = make_mod(root.path(), "a", "A", None);
        let second = make_mod(root.path(), "b", "B", None);
        for (path, label) in [(&first, "A"), (&second, "B")] {
            std::fs::create_dir_all(path.join("Defs")).unwrap();
            std::fs::write(path.join("Defs/shared.xml"), format!("<Defs><ThingDef><defName>Steel</defName><label>{label}</label></ThingDef><SongDef><label>unnamed</label></SongDef></Defs>")).unwrap();
        }
        let report = analyze_json(serde_json::json!({"mod_paths": [first, second]}));
        assert_eq!(report["schema_version"], 3);
        assert!(
            report["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["type"] == "def" && c["severity"] == "error")
        );
        assert!(
            report["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["severity"] == "info" && d["analysis_incomplete"] == true)
        );
    }

    #[test]
    fn omitted_and_valid_config_fields_keep_expected_defaults() {
        let omitted = analyze_json(serde_json::json!({}));
        assert_eq!(omitted["metadata"]["game_version"], serde_json::Value::Null);
        assert_eq!(omitted["metadata"]["order_aware"], false);

        let valid = analyze_json(serde_json::json!({
            "mod_paths": [], "game_version": "1.7.4976 rev44", "ordered": true
        }));
        assert_eq!(valid["metadata"]["game_version"], "1.7.4976 rev44");
        assert_eq!(valid["metadata"]["order_aware"], true);
    }

    #[test]
    fn invalid_known_field_types_return_json_errors() {
        for config in [
            serde_json::json!({"mod_paths": "bad"}),
            serde_json::json!({"mod_paths": ["valid", 7]}),
            serde_json::json!({"game_version": 17}),
            serde_json::json!({"game_version": null}),
            serde_json::json!({"ordered": "yes"}),
        ] {
            assert!(analyze_json(config).get("error").is_some());
        }
    }

    #[test]
    fn unknown_config_keys_warn_without_leaking_values_or_stopping_scan() {
        let root = TempDir::new();
        let path = make_mod(root.path(), "example.mod", "Example", None);
        let report = analyze_json(serde_json::json!({
            "mod_paths": [path], "mystery_z": "secret-value", "mystery_a": true
        }));
        assert_eq!(
            report["metadata"]["mods_scanned"].as_array().unwrap().len(),
            1
        );
        let diagnostics = report["diagnostics"].as_array().unwrap();
        let warning = diagnostics
            .iter()
            .find(|d| d["source"] == "metadata_resolution")
            .unwrap();
        assert_eq!(warning["severity"], "warning");
        assert!(
            warning["message"]
                .as_str()
                .unwrap()
                .find("mystery_a")
                .unwrap()
                < warning["message"]
                    .as_str()
                    .unwrap()
                    .find("mystery_z")
                    .unwrap()
        );
        assert!(
            !warning["message"]
                .as_str()
                .unwrap()
                .contains("secret-value")
        );
    }

    #[test]
    fn all_supplied_mods_are_active_for_loadfolders_conditions() {
        let root = TempDir::new();
        let condition = make_mod(
            root.path(),
            "conditional.mod",
            "Conditional",
            Some(
                "<loadFolders><v1.5><li IfModActive=\"active.mod\">Selected</li></v1.5></loadFolders>",
            ),
        );
        let active = make_mod(root.path(), "active.mod", "Active", None);
        std::fs::create_dir_all(condition.join("Selected/Defs")).unwrap();
        std::fs::write(condition.join("Selected/Defs/broken.xml"), "<broken").unwrap();
        std::fs::create_dir_all(condition.join("Defs")).unwrap();
        let report = analyze_json(serde_json::json!({
            "mod_paths": [condition, active], "game_version": "1.5.4104 rev1"
        }));
        assert!(
            report["diagnostics"].as_array().unwrap().iter().any(|d| {
                d["source"] == "xml_parsing" && d["affected_mod"] == "conditional.mod"
            }),
            "selected conditional path should be scanned: {report}"
        );
    }

    #[test]
    fn ffi_reports_folder_resolution_diagnostics() {
        let root = TempDir::new();
        let path = make_mod(
            root.path(),
            "broken.mod",
            "Broken",
            Some("<loadFolders><v1.5><li>Broken"),
        );
        let report = analyze_json(serde_json::json!({
            "mod_paths": [path], "game_version": "1.5.4104 rev1"
        }));
        assert!(report["diagnostics"].as_array().unwrap().iter().any(|d| {
            d["source"] == "folder_resolution"
                && d["severity"] == "warning"
                && d["affected_mod"] == "broken.mod"
        }));
    }

    #[test]
    fn ffi_marks_ludeon_mods_as_base_content() {
        let root = TempDir::new();
        let path = make_mod(root.path(), "ludeon.rimworld", "Core", None);
        let report = analyze_json(serde_json::json!({"mod_paths": [path]}));
        assert_eq!(
            report["metadata"]["mods_scanned"][0]["is_base_content"],
            true
        );
    }

    #[test]
    fn rcd_analyze_reports_unparseable_mod_paths() {
        let config = serde_json::json!({
            "mod_paths": ["/nonexistent/mod/dir"],
            "game_version": "1.5",
            "ordered": false
        });
        let config_c = CString::new(config.to_string()).unwrap();

        let ptr = unsafe { rcd_analyze(config_c.as_ptr(), None) };
        assert!(!ptr.is_null());

        let json = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string();
        unsafe { rcd_free(ptr) };

        let report: serde_json::Value = serde_json::from_str(&json).unwrap();
        let diagnostics = report["diagnostics"]
            .as_array()
            .expect("report must have diagnostics");
        assert!(
            diagnostics.iter().any(|d| d["severity"] == "error"
                && d["analysis_incomplete"] == true
                && d["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("Failed to parse mod"))),
            "parse failure must be visible to C# consumers: {}",
            json
        );
    }
}
