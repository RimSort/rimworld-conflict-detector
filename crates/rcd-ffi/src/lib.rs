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
        for path in &mod_paths {
            if let Ok(mut m) = rcd_core::about_xml::parse_about_xml(path) {
                rcd_core::folder::resolve_mod_folders(&mut m, game_version, None);
                mods.push(m);
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
            timestamp: String::new(), // caller can set this
            game_version: game_version.map(String::from),
            order_aware: load_order.is_some(),
            mods_scanned: mods.iter().map(rcd_core::ModSummary::from).collect(),
        };

        let report = engine.analyze(&mods, load_order.as_ref(), metadata, progress_fn.as_ref());

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
    let json = format!(
        r#"{{"error":"{}"}}"#,
        msg.replace('"', "\\\"").replace('\n', "\\n")
    );
    CString::new(json)
        .unwrap_or_else(|_| CString::new(r#"{"error":"unknown"}"#).unwrap())
        .into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

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
}
