// RimWorld Conflict Detector - FFI surface

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

pub type ProgressCallback = extern "C" fn(current: u32, total: u32, phase: *const c_char);

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

        // Parse the config JSON
        // Expected structure: { "mod_paths": ["/path/to/mod1", ...], "game_version": "1.5", "ordered": false }
        let config: serde_json::Value = match serde_json::from_str(&config_str) {
            Ok(v) => v,
            Err(e) => return error_json(&format!("Invalid config JSON: {}", e)),
        };

        // Extract mod paths
        let mod_paths: Vec<std::path::PathBuf> = config["mod_paths"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(std::path::PathBuf::from))
            .collect();

        let game_version = config["game_version"].as_str();
        let ordered = config["ordered"].as_bool().unwrap_or(false);

        // Resolve mods
        let mut mods = Vec::new();
        let mut pre_diagnostics: Vec<rcd_core::Diagnostic> = Vec::new();
        for path in &mod_paths {
            match rcd_core::about_xml::parse_about_xml(path) {
                Ok(mut m) => {
                    rcd_core::folder::resolve_mod_folders(&mut m, game_version, None);
                    mods.push(m);
                }
                Err(e) => pre_diagnostics.push(rcd_core::Diagnostic {
                    affected_mod: None,
                    severity: rcd_core::Severity::Error,
                    message: format!("Failed to parse mod at {}: {}", path.display(), e),
                    source: rcd_core::DiagnosticSource::MetadataResolution,
                }),
            }
        }

        let load_order = if ordered {
            Some(rcd_core::LoadOrder {
                package_ids: mods.iter().map(|m| m.package_id.clone()).collect(),
            })
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
        assert_eq!(parsed["schema_version"], 1);
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
                && d["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("Failed to parse mod"))),
            "parse failure must be visible to C# consumers: {}",
            json
        );
    }
}
