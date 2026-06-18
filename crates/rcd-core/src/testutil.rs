use crate::types::ModSummary;
use std::path::PathBuf;

pub fn test_mod_summary(id: &str) -> ModSummary {
    ModSummary {
        package_id: id.to_string(),
        name: id.to_string(),
        author: None,
        is_base_content: false,
    }
}

pub fn test_mod_summary_named(id: &str, name: &str) -> ModSummary {
    ModSummary {
        package_id: id.to_string(),
        name: name.to_string(),
        author: None,
        is_base_content: false,
    }
}

pub fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/mods")
}
