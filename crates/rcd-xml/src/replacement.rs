use std::collections::HashMap;

use rcd_core::{Conflict, DefConflict, DefConflictMechanism, DefTarget, LoadOrder, ModSummary};

use crate::def_parser::{ParsedDef, build_attrs_string};

/// A collection of parsed defs associated with a specific mod.
#[derive(Debug, Clone)]
pub struct ModDefs {
    pub mod_summary: ModSummary,
    pub package_id: String,
    pub defs: Vec<ParsedDef>,
}

/// Detects direct replacement conflicts: cases where two or more mods define
/// the same def (matching DefType + defName) with different XML content.
///
/// When a load order is provided, the winner is determined as the last mod
/// in the load order (RimWorld's last-wins semantics).
pub fn detect_replacements(all_mod_defs: &[ModDefs], order: Option<&LoadOrder>) -> Vec<Conflict> {
    // Build a map: (def_type, def_name) -> Vec<(ModSummary, package_id, raw_xml)>
    type DefEntries = Vec<(ModSummary, String, String)>;
    let mut def_map: HashMap<(String, String), DefEntries> = HashMap::new();

    for mod_defs in all_mod_defs {
        for def in &mod_defs.defs {
            let key = (def.def_type.clone(), def.def_name.clone());
            def_map.entry(key).or_default().push((
                mod_defs.mod_summary.clone(),
                mod_defs.package_id.clone(),
                def.raw_xml.clone(),
            ));
        }
    }

    let mut conflicts = Vec::new();

    for ((def_type, def_name), entries) in &def_map {
        if entries.len() < 2 {
            continue;
        }

        // Normalize whitespace for comparison: collapse all runs of whitespace to a single space
        let normalized: Vec<String> = entries
            .iter()
            .map(|(_, _, xml)| normalize_whitespace(xml))
            .collect();

        // Check if all normalized XMLs are identical
        let all_identical = normalized.windows(2).all(|w| w[0] == w[1]);
        if all_identical {
            continue;
        }

        // Find divergent fields by comparing the first entry against each subsequent one
        let divergent_fields = find_divergent_fields(entries);

        let mods: Vec<ModSummary> = entries.iter().map(|(ms, _, _)| ms.clone()).collect();

        let winner = order.and_then(|lo| {
            entries
                .iter()
                .filter_map(|(ms, pkg_id, _)| lo.position(pkg_id).map(|pos| (ms, pos)))
                .max_by_key(|(_, pos)| *pos)
                .map(|(ms, _)| ms.package_id.clone())
        });

        conflicts.push(Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: def_type.clone(),
                def_name: def_name.clone(),
            },
            mods,
            mechanism: DefConflictMechanism::DirectReplacement { divergent_fields },
            winner,
            condition: None,
        }));
    }

    conflicts
}

/// Normalizes an XML string for semantic comparison by re-parsing it and
/// rebuilding a canonical form. This strips insignificant whitespace between
/// tags so that formatting differences don't trigger false conflicts.
fn normalize_whitespace(s: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(s);
    reader.config_mut().trim_text(true);

    let mut result = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs = build_attrs_string(e);
                result.push_str(&format!("<{tag}{attrs}>"));
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                result.push_str(&format!("</{tag}>"));
            }
            Ok(Event::Text(ref t)) => {
                if let Ok(text) = t.unescape() {
                    let trimmed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    result.push_str(&trimmed);
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs = build_attrs_string(e);
                result.push_str(&format!("<{tag}{attrs}/>"));
            }
            Ok(Event::Eof) => break,
            Err(_) => return s.split_whitespace().collect::<Vec<_>>().join(" "),
            _ => {}
        }
        buf.clear();
    }

    result
}

/// Finds fields that differ between def entries by doing a simple line-level comparison.
/// Returns a list of tag names that appear to differ.
fn find_divergent_fields(entries: &[(ModSummary, String, String)]) -> Vec<String> {
    if entries.len() < 2 {
        return Vec::new();
    }

    let base_normalized = normalize_whitespace(&entries[0].2);
    let mut divergent = Vec::new();

    for entry in &entries[1..] {
        let other_normalized = normalize_whitespace(&entry.2);
        if base_normalized != other_normalized {
            // Simple heuristic: find top-level child tags that differ
            let base_tags = extract_child_tags(&entries[0].2);
            let other_tags = extract_child_tags(&entry.2);

            for tag in &base_tags {
                let base_content = extract_tag_content(&entries[0].2, tag);
                let other_content = extract_tag_content(&entry.2, tag);
                if base_content != other_content && !divergent.contains(tag) {
                    divergent.push(tag.clone());
                }
            }
            // Check for tags present in other but not in base
            for tag in &other_tags {
                if !base_tags.contains(tag) && !divergent.contains(tag) {
                    divergent.push(tag.clone());
                }
            }
        }
    }

    divergent
}

/// Extracts immediate child tag names from a def's raw XML.
/// Very simple: looks for `<TagName>` patterns at the first nesting level.
fn extract_child_tags(xml: &str) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut tags = Vec::new();
    let mut depth: u32 = 0;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                // depth == 2 means direct child of the root def element
                if depth == 2 {
                    let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    if !tags.contains(&tag) {
                        tags.push(tag);
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                // Self-closing tags at depth 1 (which would be depth 2 if they had content)
                if depth == 1 {
                    let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    if !tags.contains(&tag) {
                        tags.push(tag);
                    }
                }
            }
            Ok(Event::End(_)) => {
                depth -= 1;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    tags
}

/// Extracts the content of a specific child tag from a def's raw XML.
/// Returns the normalized inner text/XML for comparison.
fn extract_tag_content(xml: &str, target_tag: &str) -> Option<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut depth: u32 = 0;
    let mut capture_depth: Option<u32> = None;
    let mut captured = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if depth == 2 && tag == target_tag {
                    capture_depth = Some(depth);
                    captured.clear();
                } else if capture_depth.is_some() {
                    captured.push_str(&format!("<{tag}>"));
                }
            }
            Ok(Event::Text(ref t)) => {
                if capture_depth.is_some()
                    && let Ok(text) = t.unescape()
                {
                    captured.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if capture_depth == Some(depth) && tag == target_tag {
                    return Some(normalize_whitespace(&captured));
                }
                if capture_depth.is_some() {
                    captured.push_str(&format!("</{tag}>"));
                }
                depth -= 1;
            }
            Ok(Event::Empty(ref e)) => {
                if capture_depth.is_some() {
                    let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    captured.push_str(&format!("<{tag}/>"));
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_mod_summary(id: &str, name: &str) -> ModSummary {
        ModSummary {
            package_id: id.to_string(),
            name: name.to_string(),
            author: None,
            is_base_content: false,
        }
    }

    #[test]
    fn detect_direct_replacement() {
        let mod_a = ModDefs {
            mod_summary: test_mod_summary("test.mod.a", "Mod A"),
            package_id: "test.mod.a".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>100</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary("test.mod.b", "Mod B"),
            package_id: "test.mod.b".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>200</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let conflicts = detect_replacements(&[mod_a, mod_b], None);
        assert_eq!(conflicts.len(), 1);

        match &conflicts[0] {
            Conflict::Def(dc) => {
                assert_eq!(dc.target.def_type, "ThingDef");
                assert_eq!(dc.target.def_name, "Steel");
                assert_eq!(dc.mods.len(), 2);
                assert!(matches!(
                    &dc.mechanism,
                    DefConflictMechanism::DirectReplacement { .. }
                ));
                assert!(dc.winner.is_none());
            }
            _ => panic!("Expected Conflict::Def"),
        }
    }

    #[test]
    fn identical_defs_no_conflict() {
        let xml = "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>100</MaxHitPoints></statBases></ThingDef>";

        let mod_a = ModDefs {
            mod_summary: test_mod_summary("test.mod.a", "Mod A"),
            package_id: "test.mod.a".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: xml.to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary("test.mod.b", "Mod B"),
            package_id: "test.mod.b".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: xml.to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let conflicts = detect_replacements(&[mod_a, mod_b], None);
        assert_eq!(
            conflicts.len(),
            0,
            "Identical defs should not produce a conflict"
        );
    }

    #[test]
    fn replacement_with_load_order_reports_winner() {
        let mod_a = ModDefs {
            mod_summary: test_mod_summary("test.mod.a", "Mod A"),
            package_id: "test.mod.a".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>100</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary("test.mod.b", "Mod B"),
            package_id: "test.mod.b".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>200</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let load_order = LoadOrder {
            package_ids: vec!["test.mod.a".to_string(), "test.mod.b".to_string()],
        };

        let conflicts = detect_replacements(&[mod_a, mod_b], Some(&load_order));
        assert_eq!(conflicts.len(), 1);

        match &conflicts[0] {
            Conflict::Def(dc) => {
                // mod_b is later in load order, so it wins
                assert_eq!(
                    dc.winner.as_deref(),
                    Some("test.mod.b"),
                    "Last mod in load order should be the winner"
                );
            }
            _ => panic!("Expected Conflict::Def"),
        }
    }

    #[test]
    fn whitespace_differences_ignored() {
        let xml_a = "<ThingDef>\n  <defName>Steel</defName>\n  <statBases>\n    <MaxHitPoints>100</MaxHitPoints>\n  </statBases>\n</ThingDef>";
        let xml_b = "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>100</MaxHitPoints></statBases></ThingDef>";

        let mod_a = ModDefs {
            mod_summary: test_mod_summary("test.mod.a", "Mod A"),
            package_id: "test.mod.a".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: xml_a.to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary("test.mod.b", "Mod B"),
            package_id: "test.mod.b".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: xml_b.to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let conflicts = detect_replacements(&[mod_a, mod_b], None);
        assert_eq!(
            conflicts.len(),
            0,
            "Whitespace-only differences should not produce a conflict"
        );
    }

    #[test]
    fn divergent_fields_detected() {
        let mod_a = ModDefs {
            mod_summary: test_mod_summary("test.mod.a", "Mod A"),
            package_id: "test.mod.a".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><label>steel</label><statBases><MaxHitPoints>100</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary("test.mod.b", "Mod B"),
            package_id: "test.mod.b".to_string(),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><label>reinforced steel</label><statBases><MaxHitPoints>200</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let conflicts = detect_replacements(&[mod_a, mod_b], None);
        assert_eq!(conflicts.len(), 1);

        match &conflicts[0] {
            Conflict::Def(dc) => match &dc.mechanism {
                DefConflictMechanism::DirectReplacement { divergent_fields } => {
                    assert!(
                        divergent_fields.contains(&"label".to_string()),
                        "Should detect label as divergent"
                    );
                    assert!(
                        divergent_fields.contains(&"statBases".to_string()),
                        "Should detect statBases as divergent"
                    );
                }
                _ => panic!("Expected DirectReplacement mechanism"),
            },
            _ => panic!("Expected Conflict::Def"),
        }
    }
}
