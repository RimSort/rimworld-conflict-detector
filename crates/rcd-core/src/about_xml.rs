use crate::types::Mod;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::path::Path;
use thiserror::Error;

#[cfg(test)]
use std::path::PathBuf;

#[derive(Error, Debug)]
pub enum AboutXmlError {
    #[error("failed to read About.xml: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse About.xml: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("missing required field: {0}")]
    MissingField(String),
}

/// Parse About.xml from a mod's root directory.
///
/// Reads the `About/About.xml` file and extracts mod metadata including:
/// - packageId (required, normalized to lowercase)
/// - name (defaults to packageId if not present)
/// - author (optional)
/// - supportedVersions
/// - modDependencies (normalized to lowercase)
/// - incompatibleWith (normalized to lowercase)
/// - loadAfter, loadBefore, forceLoadAfter, forceLoadBefore (all normalized to lowercase)
pub fn parse_about_xml(mod_root: &Path) -> Result<Mod, AboutXmlError> {
    let about_path = mod_root.join("About/About.xml");
    let xml_content = std::fs::read_to_string(&about_path)?;
    parse_about_xml_str(&xml_content, mod_root)
}

/// Parse About.xml from a string (for testing).
fn parse_about_xml_str(xml: &str, mod_root: &Path) -> Result<Mod, AboutXmlError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut package_id: Option<String> = None;
    let mut name: Option<String> = None;
    let mut author: Option<String> = None;
    let mut supported_versions = Vec::new();
    let mut dependencies = Vec::new();
    let mut incompatibilities = Vec::new();
    let mut load_after = Vec::new();
    let mut load_before = Vec::new();
    let mut force_load_after = Vec::new();
    let mut force_load_before = Vec::new();

    let mut buf = Vec::new();
    let mut current_section: Option<String> = None;
    let mut in_dependency_li = false;
    let mut dep_li_element: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag_name.as_str() {
                    "supportedVersions" | "modDependencies" | "incompatibleWith" | "loadAfter"
                    | "loadBefore" | "forceLoadAfter" | "forceLoadBefore" => {
                        current_section = Some(tag_name);
                    }
                    "li" => {
                        if matches!(current_section.as_deref(), Some("modDependencies")) {
                            in_dependency_li = true;
                            dep_li_element = None;
                        }
                    }
                    _ => {
                        if in_dependency_li {
                            dep_li_element = Some(tag_name);
                        }
                    }
                }
            }
            Event::End(e) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag_name.as_str() {
                    "supportedVersions" | "modDependencies" | "incompatibleWith" | "loadAfter"
                    | "loadBefore" | "forceLoadAfter" | "forceLoadBefore" => {
                        current_section = None;
                    }
                    "li" => {
                        in_dependency_li = false;
                        dep_li_element = None;
                    }
                    _ => {
                        if in_dependency_li {
                            dep_li_element = None;
                        }
                    }
                }
            }
            Event::Text(e) => {
                let text = e.unescape()?.trim().to_string();
                if text.is_empty() {
                    continue;
                }

                // Determine which field this text belongs to
                match current_section.as_deref() {
                    Some("supportedVersions") => {
                        supported_versions.push(text);
                    }
                    Some("incompatibleWith") => {
                        incompatibilities.push(text.to_lowercase());
                    }
                    Some("loadAfter") => {
                        load_after.push(text.to_lowercase());
                    }
                    Some("loadBefore") => {
                        load_before.push(text.to_lowercase());
                    }
                    Some("forceLoadAfter") => {
                        force_load_after.push(text.to_lowercase());
                    }
                    Some("forceLoadBefore") => {
                        force_load_before.push(text.to_lowercase());
                    }
                    Some("modDependencies")
                        if in_dependency_li
                            && matches!(dep_li_element.as_deref(), Some("packageId")) =>
                    {
                        dependencies.push(text.to_lowercase());
                    }
                    None => {
                        // Top-level fields handled in second pass
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Second pass for top-level fields (packageId, name, author)
    // This is necessary because we need to track both the element name and its text content
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut current_element: Option<String> = None;
    let mut depth = 0;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                depth += 1;
                if depth == 2 {
                    // Direct child of ModMetaData
                    current_element = Some(String::from_utf8_lossy(e.name().as_ref()).to_string());
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 1 {
                    current_element = None;
                }
            }
            Event::Text(e) => {
                let text = e.unescape()?.trim().to_string();
                if text.is_empty() {
                    continue;
                }

                match current_element.as_deref() {
                    Some("packageId") => {
                        package_id = Some(text.to_lowercase());
                    }
                    Some("name") => {
                        name = Some(text);
                    }
                    Some("author") => {
                        author = Some(text);
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Validate required fields
    let package_id = package_id.ok_or_else(|| AboutXmlError::MissingField("packageId".into()))?;
    let name = name.unwrap_or_else(|| package_id.clone());

    Ok(Mod {
        package_id,
        name,
        author,
        supported_versions,
        dependencies,
        incompatibilities,
        load_after,
        load_before,
        force_load_after,
        force_load_before,
        is_base_content: false,
        root_path: mod_root.to_path_buf(),
        defs_paths: Vec::new(),
        patches_paths: Vec::new(),
        assemblies_paths: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixtures_path;

    #[test]
    fn parse_about_xml_basic() {
        let mod_root = fixtures_path().join("mod_a");
        let result = parse_about_xml(&mod_root).expect("Failed to parse About.xml");

        assert_eq!(result.package_id, "test.mod.a");
        assert_eq!(result.name, "Test Mod A");
        assert_eq!(result.author, Some("TestAuthor".to_string()));
        assert_eq!(result.supported_versions, vec!["1.4", "1.5"]);
        assert_eq!(result.dependencies, vec!["brrainz.harmony"]);
        assert_eq!(result.incompatibilities, vec!["bad.mod"]);
        assert_eq!(result.load_after, vec!["ludeon.rimworld"]);
        assert_eq!(result.load_before, vec!["other.mod"]);
        assert_eq!(result.force_load_after, vec!["ludeon.rimworld"]);
        assert!(!result.is_base_content);
        assert_eq!(result.root_path, mod_root);
    }

    #[test]
    fn parse_about_xml_missing_package_id() {
        let xml = r#"
<?xml version="1.0" encoding="utf-8"?>
<ModMetaData>
    <name>Test Mod</name>
</ModMetaData>
        "#;

        let mod_root = PathBuf::from("/fake/path");
        let result = parse_about_xml_str(xml, &mod_root);

        assert!(result.is_err());
        match result {
            Err(AboutXmlError::MissingField(field)) => {
                assert_eq!(field, "packageId");
            }
            _ => panic!("Expected MissingField error"),
        }
    }

    #[test]
    fn package_id_normalized_to_lowercase() {
        let xml = r#"
<?xml version="1.0" encoding="utf-8"?>
<ModMetaData>
    <packageId>Test.Mod.MixedCase</packageId>
    <name>Test Mod</name>
    <modDependencies>
        <li>
            <packageId>Dependency.WithCaps</packageId>
        </li>
    </modDependencies>
    <incompatibleWith>
        <li>Bad.Mod.Caps</li>
    </incompatibleWith>
    <loadAfter>
        <li>LoadAfter.Caps</li>
    </loadAfter>
</ModMetaData>
        "#;

        let mod_root = PathBuf::from("/fake/path");
        let result = parse_about_xml_str(xml, &mod_root).expect("Failed to parse");

        assert_eq!(result.package_id, "test.mod.mixedcase");
        assert_eq!(result.dependencies, vec!["dependency.withcaps"]);
        assert_eq!(result.incompatibilities, vec!["bad.mod.caps"]);
        assert_eq!(result.load_after, vec!["loadafter.caps"]);
    }

    #[test]
    fn dependency_only_captures_package_id() {
        let xml = r#"
<?xml version="1.0" encoding="utf-8"?>
<ModMetaData>
    <packageId>test.mod</packageId>
    <modDependencies>
        <li>
            <packageId>brrainz.harmony</packageId>
            <displayName>Harmony</displayName>
            <steamWorkshopUrl>steam://url/CommunityFilePage/2009463077</steamWorkshopUrl>
            <downloadUrl>https://github.com/pardeike/HarmonyRimWorld/releases/latest</downloadUrl>
        </li>
        <li>
            <packageId>other.dep</packageId>
            <displayName>Other Dependency</displayName>
        </li>
    </modDependencies>
</ModMetaData>
        "#;

        let mod_root = PathBuf::from("/fake/path");
        let result = parse_about_xml_str(xml, &mod_root).expect("Failed to parse");

        assert_eq!(
            result.dependencies,
            vec!["brrainz.harmony", "other.dep"],
            "Should only capture packageId, not displayName or URLs"
        );
    }
}
