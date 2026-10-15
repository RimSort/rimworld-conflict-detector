//! Pure parsing and selection for a mod's `LoadFolders.xml`.

use std::path::{Component, Path, PathBuf};

use crate::Severity;
use crate::diagnostic::{Diagnostic, DiagnosticSource};
use crate::xml_io::read_xml_file;
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModIdentity {
    pub package_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadFolderEntry {
    /// Original folder text, retained exactly (including meaningful spaces).
    pub path: String,
    pub required_any_of: Vec<String>,
    pub required_all_of: Vec<String>,
    pub disallowed_any_of: Vec<String>,
}

impl LoadFolderEntry {
    /// Returns the validated mod-relative path; root markers map to the empty path.
    pub fn relative_path(&self) -> Option<PathBuf> {
        validate_folder_path(&self.path).ok()
    }

    pub fn matches(&self, active_mods: &[ModIdentity]) -> bool {
        (self.required_any_of.is_empty()
            || self
                .required_any_of
                .iter()
                .any(|id| active_mods.iter().any(|m| identity_matches(id, m))))
            && self
                .required_all_of
                .iter()
                .all(|id| active_mods.iter().any(|m| identity_matches(id, m)))
            && !self
                .disallowed_any_of
                .iter()
                .any(|id| active_mods.iter().any(|m| identity_matches(id, m)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadFolders {
    pub default_entries: Option<Vec<LoadFolderEntry>>,
    pub version_entries: Vec<(String, Vec<LoadFolderEntry>)>,
    /// Unknown condition attributes, in source order (deduplicated at parse time).
    pub unknown_conditions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FolderSelection {
    /// `None` means use the existing implicit resolver; `Some(vec![])` is a deliberate empty selection.
    pub entries: Option<Vec<LoadFolderEntry>>,
    pub diagnostic: Option<Diagnostic>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadFoldersError {
    #[error("failed to read LoadFolders.xml: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse LoadFolders.xml: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("LoadFolders.xml root element is missing or invalid")]
    InvalidRoot,
}

impl LoadFolders {
    pub fn parse_file(path: &Path) -> Result<Self, LoadFoldersError> {
        parse_load_folders(&read_xml_file(path)?)
    }

    pub fn parse_str(xml: &str) -> Result<Self, LoadFoldersError> {
        parse_load_folders(xml)
    }

    pub fn select(
        &self,
        requested_version: Option<&str>,
        active_mods: &[ModIdentity],
    ) -> FolderSelection {
        select_folders(self, requested_version, active_mods)
    }

    /// Parse and select a file, converting read/parse failures into an implicit-fallback warning.
    pub fn resolve_file(
        path: &Path,
        requested_version: Option<&str>,
        active_mods: &[ModIdentity],
    ) -> FolderSelection {
        match Self::parse_file(path) {
            Ok(folders) => folders.select(requested_version, active_mods),
            Err(error) => FolderSelection {
                entries: None,
                diagnostic: Some(folder_warning(format!(
                    "Failed to parse LoadFolders.xml; using implicit folder resolution: {error}"
                ))),
            },
        }
    }
}

fn parse_load_folders(xml: &str) -> Result<LoadFolders, LoadFoldersError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut in_root = false;
    let mut root_closed = false;
    let mut current_version: Option<String> = None;
    let mut current_entry: Option<LoadFolderEntry> = None;
    let mut current_entry_element: Option<String> = None;
    let mut entry_depth = 0usize;
    let mut text = String::new();
    let mut result = LoadFolders::default();
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                if root_closed {
                    return Err(LoadFoldersError::InvalidRoot);
                }
                let name = String::from_utf8_lossy(start.local_name().as_ref()).to_lowercase();
                if !in_root {
                    if name != "loadfolders" {
                        return Err(LoadFoldersError::InvalidRoot);
                    }
                    in_root = true;
                } else if current_entry.is_some() {
                    entry_depth += 1;
                } else if let Some(version) = name.strip_prefix('v') {
                    if normalize_game_version(version).is_some() {
                        current_version = Some(version.to_owned());
                        ensure_version(&mut result, version);
                    } else if name == "default" {
                        current_version = Some("default".to_owned());
                        result.default_entries.get_or_insert_with(Vec::new);
                    } else {
                        current_entry = Some(parse_entry(
                            &start,
                            &reader,
                            &mut result.unknown_conditions,
                        )?);
                        current_entry_element =
                            Some(String::from_utf8_lossy(start.local_name().as_ref()).into_owned());
                        entry_depth = 1;
                        text.clear();
                    }
                } else if name == "default" {
                    current_version = Some("default".to_owned());
                    result.default_entries.get_or_insert_with(Vec::new);
                } else {
                    current_entry = Some(parse_entry(
                        &start,
                        &reader,
                        &mut result.unknown_conditions,
                    )?);
                    current_entry_element =
                        Some(String::from_utf8_lossy(start.local_name().as_ref()).into_owned());
                    entry_depth = 1;
                    text.clear();
                }
            }
            Event::Empty(start) => {
                if root_closed {
                    return Err(LoadFoldersError::InvalidRoot);
                }
                let name = String::from_utf8_lossy(start.local_name().as_ref()).to_lowercase();
                if !in_root {
                    if name != "loadfolders" {
                        return Err(LoadFoldersError::InvalidRoot);
                    }
                    in_root = true;
                    root_closed = true;
                } else if current_version.is_some() {
                    let entry = parse_entry(&start, &reader, &mut result.unknown_conditions)?;
                    append_entry(&mut result, current_version.as_deref().unwrap(), entry);
                } else if name == "default" {
                    result.default_entries.get_or_insert_with(Vec::new);
                } else if let Some(version) = name
                    .strip_prefix('v')
                    .filter(|v| normalize_game_version(v).is_some())
                {
                    ensure_version(&mut result, version);
                } else if current_version.is_some() {
                    let entry = parse_entry(&start, &reader, &mut result.unknown_conditions)?;
                    append_entry(&mut result, current_version.as_deref().unwrap(), entry);
                }
            }
            Event::Text(value) if current_entry.is_some() => text.push_str(&value.unescape()?),
            Event::Text(value) if root_closed && !value.unescape()?.trim().is_empty() => {
                return Err(LoadFoldersError::InvalidRoot);
            }
            Event::CData(value) if current_entry.is_some() => {
                text.push_str(&String::from_utf8_lossy(&value))
            }
            Event::End(end) => {
                let local_name = String::from_utf8_lossy(end.local_name().as_ref()).into_owned();
                let name = local_name.to_lowercase();
                if current_entry.is_some() {
                    if entry_depth > 1 {
                        entry_depth -= 1;
                    } else if current_entry_element
                        .as_deref()
                        .is_some_and(|tag| tag == local_name.as_str())
                    {
                        let mut entry = current_entry.take().unwrap();
                        current_entry_element = None;
                        entry_depth = 0;
                        entry.path = text.clone();
                        if let Some(version) = current_version.as_deref() {
                            append_entry(&mut result, version, entry);
                        }
                        text.clear();
                    }
                } else if name == "default"
                    || (name.starts_with('v')
                        && current_version
                            .as_deref()
                            .is_some_and(|v| name.eq_ignore_ascii_case(&format!("v{v}"))))
                {
                    current_version = None;
                } else if name == "loadfolders" {
                    root_closed = true;
                } else if root_closed {
                    return Err(LoadFoldersError::InvalidRoot);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !in_root || !root_closed || current_entry.is_some() {
        return Err(LoadFoldersError::InvalidRoot);
    }
    Ok(result)
}

fn parse_entry(
    start: &quick_xml::events::BytesStart<'_>,
    reader: &Reader<&[u8]>,
    unknown: &mut Vec<String>,
) -> Result<LoadFolderEntry, LoadFoldersError> {
    let mut entry = LoadFolderEntry::default();
    for attr in start.attributes() {
        let attr = attr.map_err(quick_xml::Error::from)?;
        let name = String::from_utf8_lossy(attr.key.local_name().as_ref()).into_owned();
        let value = attr
            .decode_and_unescape_value(reader.decoder())?
            .into_owned();
        let values = split_ids(&value);
        match name.as_str() {
            "IfModActive" => entry.required_any_of.extend(values),
            "IfModActiveAll" => entry.required_all_of.extend(values),
            "IfModNotActive" => entry.disallowed_any_of.extend(values),
            _ => {
                if !unknown.contains(&name) {
                    unknown.push(name);
                }
            }
        }
    }
    Ok(entry)
}

fn split_ids(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn ensure_version(folders: &mut LoadFolders, version: &str) {
    if !folders
        .version_entries
        .iter()
        .any(|(v, _)| v.eq_ignore_ascii_case(version))
    {
        folders
            .version_entries
            .push((version.to_owned(), Vec::new()));
    }
}
fn append_entry(folders: &mut LoadFolders, version: &str, entry: LoadFolderEntry) {
    if version.eq_ignore_ascii_case("default") {
        folders
            .default_entries
            .get_or_insert_with(Vec::new)
            .push(entry);
    } else {
        ensure_version(folders, version);
        if let Some((_, entries)) = folders
            .version_entries
            .iter_mut()
            .find(|(v, _)| v.eq_ignore_ascii_case(version))
        {
            entries.push(entry);
        }
    }
}
fn normalize_game_version(version: &str) -> Option<(u32, u32)> {
    let core = version.trim();
    let core = core.split_once(" rev").map_or(core, |(v, rev)| {
        if !rev.is_empty() && rev.chars().all(|c| c.is_ascii_digit()) {
            v
        } else {
            ""
        }
    });
    let parts: Vec<_> = core.split('.').collect();
    if !(parts.len() == 2 || parts.len() == 3)
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
}
fn select_folders(
    folders: &LoadFolders,
    requested: Option<&str>,
    active: &[ModIdentity],
) -> FolderSelection {
    let mut chosen: Option<Vec<LoadFolderEntry>> = None;
    if let Some(requested) = requested.and_then(normalize_game_version) {
        let best = folders
            .version_entries
            .iter()
            .filter_map(|(v, _)| normalize_game_version(v))
            .filter(|version| *version <= requested)
            .max();
        if let Some(best) = best {
            chosen = Some(
                folders
                    .version_entries
                    .iter()
                    .filter(|(version, _)| normalize_game_version(version) == Some(best))
                    .flat_map(|(_, entries)| entries.iter().cloned())
                    .collect(),
            );
        }
    }
    if chosen.is_none() {
        chosen = folders.default_entries.clone();
    }
    let Some(entries) = chosen else {
        return FolderSelection {
            entries: None,
            diagnostic: Some(folder_warning(
                "No applicable LoadFolders.xml version entry; using implicit folder resolution",
            )),
        };
    };
    let mut diagnostics = Vec::new();
    let mut selected = Vec::new();
    for entry in entries {
        if validate_folder_path(&entry.path).is_err() {
            diagnostics.push(folder_warning(format!(
                "Invalid LoadFolders.xml folder path {:?}; skipping entry",
                entry.path
            )));
        } else if entry.matches(active) {
            selected.push(entry);
        }
    }
    let diagnostic = if selected.is_empty() {
        Some(folder_warning("LoadFolders.xml selected no folders"))
    } else {
        diagnostics.into_iter().next()
    };
    FolderSelection {
        entries: Some(selected),
        diagnostic,
    }
}
fn folder_warning(message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        affected_mod: None,
        severity: Severity::Warning,
        message: message.into(),
        source: DiagnosticSource::FolderResolution,
    }
}
fn validate_folder_path(text: &str) -> Result<PathBuf, ()> {
    if text.is_empty() || text == "/" || text == "\\" {
        return Ok(PathBuf::new());
    }
    let normalized = text.replace('\\', "/");
    let path = Path::new(&normalized);
    let has_windows_prefix = normalized.as_bytes().get(1) == Some(&b':')
        && normalized.as_bytes()[0].is_ascii_alphabetic();
    if has_windows_prefix
        || path.is_absolute()
        || normalized.starts_with('/')
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(());
    }
    Ok(path.to_path_buf())
}
fn identity_matches(wanted: &str, candidate: &ModIdentity) -> bool {
    let wanted = wanted.trim();
    let package_id_matches = wanted.eq_ignore_ascii_case(&candidate.package_id)
        || wanted
            .strip_suffix("_steam")
            .is_some_and(|id| id.eq_ignore_ascii_case(&candidate.package_id))
        || candidate
            .package_id
            .strip_suffix("_steam")
            .is_some_and(|id| id.eq_ignore_ascii_case(wanted));
    package_id_matches || wanted.eq_ignore_ascii_case(&candidate.display_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mods() -> Vec<ModIdentity> {
        vec![
            ModIdentity {
                package_id: "Acme.Core_steam".into(),
                display_name: "Core Mod".into(),
            },
            ModIdentity {
                package_id: "acme.extra".into(),
                display_name: "Extra Mod".into(),
            },
        ]
    }

    #[test]
    fn parses_case_insensitive_root_and_versions_with_bom_and_duplicate_blocks() {
        let xml = "\u{feff}<LOADFOLDERS><V1.5><li> First </li></V1.5><v1.5><folder>Second</folder></v1.5></LOADFOLDERS>";
        let parsed = LoadFolders::parse_str(xml).unwrap();
        assert_eq!(parsed.version_entries.len(), 1);
        assert_eq!(
            parsed.version_entries[0]
                .1
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            [" First ", "Second"]
        );
    }

    #[test]
    fn selects_exact_then_greatest_lower_then_default_and_normalizes_versions() {
        let parsed = LoadFolders::parse_str("<loadFolders><v1.4><li>old</li></v1.4><v1.5><li>exact</li></v1.5><default><li>fallback</li></default></loadFolders>").unwrap();
        assert_eq!(
            parsed.select(Some("1.5.4085 rev442"), &[]).entries.unwrap()[0].path,
            "exact"
        );
        assert_eq!(
            parsed.select(Some("1.6.2"), &[]).entries.unwrap()[0].path,
            "exact"
        );
        assert_eq!(
            parsed.select(None, &[]).entries.unwrap()[0].path,
            "fallback"
        );
        assert_eq!(normalize_game_version("1.7.4976"), Some((1, 7)));
        assert_eq!(normalize_game_version("1.7 rev44"), Some((1, 7)));
        assert_eq!(normalize_game_version("../1.7"), None);
        assert_eq!(normalize_game_version("1.x"), None);
    }

    #[test]
    fn recognized_conditions_are_trimmed_and_anded_with_identity_matching() {
        let parsed = LoadFolders::parse_str("<loadFolders><v1.5><li IfModActive=\"missing, Core Mod \" IfModActiveAll=\"acme.extra\" IfModNotActive=\"forbidden\">ok</li><li IfModNotActive=\"ACME.CORE_STEAM\">no</li></v1.5></loadFolders>").unwrap();
        let selected = parsed.select(Some("1.5"), &mods());
        assert_eq!(
            selected
                .entries
                .unwrap()
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["ok"]
        );
    }

    #[test]
    fn steam_suffix_tolerance_applies_only_to_package_ids() {
        let parsed = LoadFolders::parse_str(
            "<loadFolders><v1.5><li IfModActive=\"Acme.Core\">package-id</li><li IfModActive=\"Core Mod_steam\">display-name</li></v1.5></loadFolders>",
        )
        .unwrap();
        let selected = parsed.select(Some("1.5"), &mods());
        assert_eq!(
            selected
                .entries
                .unwrap()
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["package-id"]
        );
    }

    #[test]
    fn nested_folder_text_is_preserved_and_following_sibling_is_intact() {
        let parsed = LoadFolders::parse_str(
            "<loadFolders><v1.5><li>one<b>two</b>three</li><folder>sibling</folder></v1.5></loadFolders>",
        )
        .unwrap();
        assert_eq!(
            parsed.version_entries[0]
                .1
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["onetwothree", "sibling"]
        );
    }

    #[test]
    fn root_markers_unknown_conditions_and_invalid_paths_are_handled() {
        let parsed = LoadFolders::parse_str("<loadFolders><default><li>/</li><li>\\</li><li></li><li IfModActiveAny=\"unused\"> spaced </li><li>../escape</li></default></loadFolders>").unwrap();
        assert_eq!(parsed.unknown_conditions, ["IfModActiveAny"]);
        assert_eq!(
            parsed.default_entries.as_ref().unwrap()[0].relative_path(),
            Some(PathBuf::new())
        );
        assert_eq!(
            parsed.default_entries.as_ref().unwrap()[1].relative_path(),
            Some(PathBuf::new())
        );
        assert_eq!(
            parsed.default_entries.as_ref().unwrap()[2].relative_path(),
            Some(PathBuf::new())
        );
        assert_eq!(
            parsed.default_entries.as_ref().unwrap()[4].relative_path(),
            None
        );
        let selection = parsed.select(None, &[]);
        assert_eq!(
            selection
                .entries
                .unwrap()
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["/", "\\", "", " spaced "]
        );
        assert_eq!(
            selection.diagnostic.unwrap().source,
            DiagnosticSource::FolderResolution
        );
    }

    #[test]
    fn malformed_and_no_version_are_distinguishable_from_empty_selected_result() {
        assert!(LoadFolders::parse_str("<loadFolders>").is_err());
        let parsed = LoadFolders::parse_str(
            "<loadFolders><v1.5><li IfModActive=\"missing\">hidden</li></v1.5></loadFolders>",
        )
        .unwrap();
        let selected = parsed.select(Some("1.5"), &[]);
        assert_eq!(selected.entries, Some(vec![]));
        assert_eq!(selected.diagnostic.unwrap().severity, Severity::Warning);
        let no_version = parsed.select(Some("1.4"), &[]);
        assert!(no_version.entries.is_none());
        assert_eq!(
            no_version.diagnostic.unwrap().source,
            DiagnosticSource::FolderResolution
        );
        assert!(LoadFolders::parse_str("<loadFolders><v1.5><li>abc</v1.5></loadFolders>").is_err());
        assert!(LoadFolders::parse_str("<loadFolders></loadFolders><other/>").is_err());
        let fallback =
            LoadFolders::resolve_file(std::path::Path::new("/missing/LoadFolders.xml"), None, &[]);
        assert!(fallback.entries.is_none());
        assert_eq!(
            fallback.diagnostic.unwrap().source,
            DiagnosticSource::FolderResolution
        );
    }

    #[test]
    fn parse_file_uses_shared_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("LoadFolders.xml");
        std::fs::write(
            &path,
            vec![b'x'; crate::xml_io::MAX_XML_FILE_BYTES as usize + 1],
        )
        .unwrap();
        assert!(LoadFolders::parse_file(&path).is_err());
    }
}
