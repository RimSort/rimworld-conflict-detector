use std::collections::{BTreeMap, HashMap};

use rcd_core::{Conflict, DefConflict, DefConflictMechanism, DefTarget, LoadOrder, ModSummary};

use crate::def_parser::ParsedDef;

/// A collection of parsed defs associated with a specific mod.
#[derive(Debug, Clone)]
pub struct ModDefs {
    pub mod_summary: ModSummary,
    pub defs: Vec<ParsedDef>,
}

/// Detects direct replacement conflicts: cases where two or more mods define
/// the same def (matching DefType + defName) with different XML content.
///
/// When a load order is provided, the winner is determined as the last mod
/// in the load order (RimWorld's last-wins semantics).
///
/// Returns `(conflicts, internal_reconstruction_errors)`; the second vec
/// surfaces Errs from parsing our own reconstructed raw_xml — a
/// reconstruction bug, not user-input malformation.
pub fn detect_replacements(
    all_mod_defs: &[ModDefs],
    order: Option<&LoadOrder>,
) -> (Vec<Conflict>, Vec<String>) {
    let (conflicts, errors) = detect_replacements_structured(all_mod_defs, order);
    (
        conflicts,
        errors.into_iter().map(|issue| issue.message).collect(),
    )
}

pub(crate) fn detect_replacements_structured(
    all_mod_defs: &[ModDefs],
    order: Option<&LoadOrder>,
) -> (Vec<Conflict>, Vec<crate::XmlIssue>) {
    // Build a map: (def_type, def_name) -> Vec<(ModSummary, raw_xml)>
    type DefEntries = Vec<(ModSummary, String)>;
    let mut def_map: HashMap<(String, String), DefEntries> = HashMap::new();

    for mod_defs in all_mod_defs {
        for def in &mod_defs.defs {
            let key = (def.def_type.clone(), def.def_name.clone());
            def_map
                .entry(key)
                .or_default()
                .push((mod_defs.mod_summary.clone(), def.raw_xml.clone()));
        }
    }

    let mut conflicts = Vec::new();
    let mut errors = Vec::new();

    for ((def_type, def_name), entries) in &def_map {
        if entries.len() < 2 {
            continue;
        }

        // Suppress false positives when a single mod provides the same def
        // from multiple folders (e.g., 1.5/Defs/ and Common/Defs/)
        let first_pkg = &entries[0].0.package_id;
        if entries.iter().all(|(ms, _)| ms.package_id == *first_pkg) {
            continue;
        }

        // Normalize whitespace for comparison: collapse all runs of whitespace to a single space
        let errors_before_normalization = errors.len();
        let normalized: Vec<String> = entries
            .iter()
            .map(|(_, xml)| {
                normalize_whitespace(xml, &format!("{def_type}:{def_name}"), &mut errors)
            })
            .collect();

        // Invalid text/XML cannot establish semantic equivalence, even when
        // the malformed source bytes happen to be identical.
        let all_identical = errors.len() == errors_before_normalization
            && normalized.windows(2).all(|w| w[0] == w[1]);
        if all_identical {
            continue;
        }

        // Find divergent fields by comparing the first entry against each subsequent one
        let divergent_fields =
            find_divergent_fields(entries, &format!("{def_type}:{def_name}"), &mut errors);

        let mods: Vec<ModSummary> = entries.iter().map(|(ms, _)| ms.clone()).collect();

        let winner = order.and_then(|lo| {
            entries
                .iter()
                .filter_map(|(ms, _)| lo.position(&ms.package_id).map(|pos| (ms, pos)))
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

    // Byte-identical reconstruction errors repeat per comparison pair and
    // def_map iteration order is nondeterministic run-to-run; canonicalize.
    errors.sort_by(|a, b| (&a.message, &a.code, &a.params).cmp(&(&b.message, &b.code, &b.params)));
    errors.dedup_by(|a, b| (&a.message, &a.code, &a.params) == (&b.message, &b.code, &b.params));

    (conflicts, errors)
}

/// Normalizes an XML string for semantic comparison by re-parsing it and
/// rebuilding a canonical form. This strips insignificant whitespace between
/// tags so that formatting differences don't trigger false conflicts.
fn normalize_whitespace(s: &str, def_id: &str, errors: &mut Vec<crate::XmlIssue>) -> String {
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
                let attrs = match canonical_attrs(e, &reader) {
                    Ok(attrs) => attrs,
                    Err(error) => {
                        errors.push(error.into_issue(def_id));
                        return format!("!invalid-xml:{s}");
                    }
                };
                result.push_str(&format!("<{tag}{attrs}>"));
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                result.push_str(&format!("</{tag}>"));
            }
            Ok(Event::Text(ref t)) => match t.unescape() {
                Ok(text) => {
                    let trimmed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    result.push_str(&trimmed);
                }
                Err(error) => {
                    errors.push(crate::XmlIssue::reconstruction_error(def_id, error, "text"));
                    return format!("!invalid-xml:{s}");
                }
            },
            Ok(Event::CData(ref c)) => {
                // CDATA content is already decoded; canonicalize it like Text
                // so CDATA-only divergences are visible to the comparison
                // (def_parser re-emits CDATA verbatim into raw_xml).
                let content = String::from_utf8_lossy(c.as_ref());
                let trimmed: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
                result.push_str(&trimmed);
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs = match canonical_attrs(e, &reader) {
                    Ok(attrs) => attrs,
                    Err(error) => {
                        errors.push(error.into_issue(def_id));
                        return format!("!invalid-xml:{s}");
                    }
                };
                result.push_str(&format!("<{tag}{attrs}/>"));
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                errors.push(crate::XmlIssue::reconstruction_error(
                    def_id, error, "parse",
                ));
                return format!("!invalid-xml:{s}");
            }
            _ => {}
        }
        buf.clear();
    }

    result
}

/// Finds fields that differ between def entries by doing a simple line-level comparison.
/// Returns a list of tag names that appear to differ.
fn find_divergent_fields(
    entries: &[(ModSummary, String)],
    def_id: &str,
    errors: &mut Vec<crate::XmlIssue>,
) -> Vec<String> {
    if entries.len() < 2 {
        return Vec::new();
    }

    let base_normalized = normalize_whitespace(&entries[0].1, def_id, errors);
    let mut divergent = Vec::new();

    for entry in &entries[1..] {
        let other_normalized = normalize_whitespace(&entry.1, def_id, errors);
        if base_normalized != other_normalized {
            let base_attributes = extract_root_attributes(&entries[0].1, def_id, errors);
            let other_attributes = extract_root_attributes(&entry.1, def_id, errors);
            // Canonical order: each map is ordered, but chaining the two key
            // sets is only sorted when both sides carry the same attributes.
            let mut attribute_names = base_attributes
                .keys()
                .chain(other_attributes.keys())
                .collect::<Vec<_>>();
            attribute_names.sort_unstable();
            attribute_names.dedup();
            for name in attribute_names {
                if base_attributes.get(name) != other_attributes.get(name) {
                    let field = format!("@{name}");
                    if !divergent.contains(&field) {
                        divergent.push(field);
                    }
                }
            }
            // Simple heuristic: find top-level child tags that differ
            let base_tags = extract_child_tags(&entries[0].1, def_id, errors);
            let other_tags = extract_child_tags(&entry.1, def_id, errors);

            for tag in &base_tags {
                let base_content = extract_tag_content(&entries[0].1, tag, def_id, errors);
                let other_content = extract_tag_content(&entry.1, tag, def_id, errors);
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

enum AttributeError {
    Syntax(String),
    Decode { attribute: String, detail: String },
}

impl AttributeError {
    fn into_issue(self, def_id: &str) -> crate::XmlIssue {
        match self {
            Self::Syntax(detail) => crate::XmlIssue::reconstruction_error(def_id, detail, "parse"),
            Self::Decode { attribute, detail } => {
                let mut issue = crate::XmlIssue::reconstruction_error(def_id, detail, "attribute");
                issue.params.insert("attribute".into(), attribute);
                issue
            }
        }
    }
}

/// Canonicalizes attributes by decoded XML value, then safely re-escapes them.
fn canonical_attrs(
    element: &quick_xml::events::BytesStart<'_>,
    reader: &quick_xml::Reader<&[u8]>,
) -> Result<String, AttributeError> {
    let mut attributes = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| AttributeError::Syntax(error.to_string()))?;
        let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| AttributeError::Decode {
                attribute: key.clone(),
                detail: error.to_string(),
            })?;
        let escaped = value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('"', "&quot;");
        attributes.push((key, escaped));
    }
    attributes.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(attributes
        .into_iter()
        .map(|(key, value)| format!(r#" {key}="{value}""#))
        .collect())
}

/// Reads the root element attributes for root-attribute field reporting.
///
/// Ordered keys: `divergent_fields` inherits this iteration order, so a map
/// with randomized ordering would make the reported field list run-dependent.
fn extract_root_attributes(
    xml: &str,
    def_id: &str,
    errors: &mut Vec<crate::XmlIssue>,
) -> BTreeMap<String, String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                let mut attributes = BTreeMap::new();
                for attribute in event.attributes() {
                    let attribute = match attribute {
                        Ok(attribute) => attribute,
                        Err(error) => {
                            errors.push(crate::XmlIssue::reconstruction_error(
                                def_id,
                                quick_xml::Error::from(error),
                                "attribute",
                            ));
                            return BTreeMap::new();
                        }
                    };
                    let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
                    let value = match attribute.decode_and_unescape_value(reader.decoder()) {
                        Ok(value) => value.into_owned(),
                        Err(error) => {
                            errors.push(
                                AttributeError::Decode {
                                    attribute: key,
                                    detail: error.to_string(),
                                }
                                .into_issue(def_id),
                            );
                            return BTreeMap::new();
                        }
                    };
                    attributes.insert(key, value);
                }
                return attributes;
            }
            Ok(Event::Eof) => return BTreeMap::new(),
            Err(error) => {
                errors.push(crate::XmlIssue::reconstruction_error(
                    def_id, error, "parse",
                ));
                return BTreeMap::new();
            }
            _ => {}
        }
        buf.clear();
    }
}

/// Extracts immediate child tag names from a def's raw XML.
/// Very simple: looks for `<TagName>` patterns at the first nesting level.
fn extract_child_tags(xml: &str, def_id: &str, errors: &mut Vec<crate::XmlIssue>) -> Vec<String> {
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
            Err(e) => {
                errors.push(crate::XmlIssue::reconstruction_error(def_id, e, "parse"));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    tags
}

/// Extracts the content of a specific child tag from a def's raw XML.
/// Returns the normalized inner text/XML for comparison.
fn extract_tag_content(
    xml: &str,
    target_tag: &str,
    def_id: &str,
    errors: &mut Vec<crate::XmlIssue>,
) -> Option<String> {
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
                if capture_depth.is_some() {
                    match t.unescape() {
                        Ok(text) => captured.push_str(&text),
                        Err(error) => {
                            errors
                                .push(crate::XmlIssue::reconstruction_error(def_id, error, "text"));
                            captured.push_str(&format!(
                                "!invalid-text:{}",
                                String::from_utf8_lossy(t.as_ref())
                            ));
                        }
                    }
                }
            }
            Ok(Event::CData(ref c)) => {
                if capture_depth.is_some() {
                    captured.push_str(&String::from_utf8_lossy(c.as_ref()));
                }
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if capture_depth == Some(depth) && tag == target_tag {
                    return Some(captured.split_whitespace().collect::<Vec<_>>().join(" "));
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
            Err(e) => {
                errors.push(crate::XmlIssue::reconstruction_error(def_id, e, "parse"));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcd_core::testutil::test_mod_summary_named;

    #[test]
    fn reconstruction_diagnostic_identity_preserves_attributes_and_coalesces_repeats() {
        for attributes in [["attr0", "attr1"], ["attr0", "attr0"]] {
            let defs = attributes
                .iter()
                .enumerate()
                .map(|(i, attr)| ModDefs {
                    mod_summary: test_mod_summary_named(&format!("mod.{i}"), &format!("Mod {i}")),
                    defs: vec![ParsedDef {
                        def_type: "ThingDef".into(),
                        def_name: "Steel".into(),
                        raw_xml: format!("<ThingDef {attr}=\"&bogus;\"/>"),
                        source_file: "a.xml".into(),
                    }],
                })
                .collect::<Vec<_>>();
            let expected = if attributes[0] == attributes[1] {
                vec!["attr0"]
            } else {
                vec!["attr0", "attr1"]
            };
            for input in [defs.clone(), defs.into_iter().rev().collect()] {
                let (_, issues) = detect_replacements_structured(&input, None);
                let actual = issues
                    .iter()
                    .filter(|issue| {
                        issue.code.as_deref() == Some("xml_reconstruction_attribute_failure")
                    })
                    .map(|issue| issue.params["attribute"].as_str())
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn reconstruction_text_failure_carries_def_and_detail() {
        let def = ParsedDef {
            def_type: "ThingDef".into(),
            def_name: "Steel".into(),
            raw_xml: "<ThingDef><label>&bogus;</label></ThingDef>".into(),
            source_file: "a.xml".into(),
        };
        let a = ModDefs {
            mod_summary: test_mod_summary_named("a", "A"),
            defs: vec![def.clone()],
        };
        let b = ModDefs {
            mod_summary: test_mod_summary_named("b", "B"),
            defs: vec![def],
        };
        let (_, issues) = detect_replacements_structured(&[a, b], None);
        let issue = issues
            .iter()
            .find(|issue| issue.code.as_deref() == Some("xml_reconstruction_text_failure"))
            .unwrap();
        assert_eq!(issue.params["def_id"], "ThingDef:Steel");
        assert!(!issue.params["detail"].is_empty());
        assert!(issue.message.starts_with(
            "internal XML reconstruction error for ThingDef:Steel: text decode failed:"
        ));
    }

    #[test]
    fn reconstruction_attribute_failure_names_the_attribute() {
        let def = ParsedDef {
            def_type: "ThingDef".into(),
            def_name: "Steel".into(),
            raw_xml: "<ThingDef Label=\"&bogus;\"/>".into(),
            source_file: "a.xml".into(),
        };
        let a = ModDefs {
            mod_summary: test_mod_summary_named("a", "A"),
            defs: vec![def.clone()],
        };
        let b = ModDefs {
            mod_summary: test_mod_summary_named("b", "B"),
            defs: vec![def],
        };
        let (_, issues) = detect_replacements_structured(&[a, b], None);
        let issue = issues
            .iter()
            .find(|issue| issue.code.as_deref() == Some("xml_reconstruction_attribute_failure"))
            .unwrap();
        assert_eq!(issue.params["attribute"], "Label");
        assert_eq!(issue.params["def_id"], "ThingDef:Steel");
        assert!(!issue.params["detail"].is_empty());
    }

    #[test]
    fn detect_replacements_reports_internal_reconstruction_errors() {
        // raw_xml is OUR reconstruction; a parse Err here means a reconstruction
        // bug, which must surface as a diagnostic rather than vanish. Well-formed
        // input must keep the errors vec empty.
        let mod_a = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName></ThingDef>".to_string(),
                source_file: "a.xml".to_string(),
            }],
        };
        let mod_b = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.b", "Mod B"),
            defs: mod_a.defs.clone(),
        };
        // DISTINCT package ids here only dodge the same-package short-circuit;
        // the identical XML then hits all_identical, which also continues before
        // find_divergent_fields — so this sub-case can never populate errors.
        // It pins "well-formed input stays quiet"; the c/d pair below is what
        // actually exercises the error-capable path (FR-2c gate M5 correction).
        let (conflicts, errors) = detect_replacements(&[mod_a, mod_b], None);
        assert!(conflicts.is_empty());
        assert!(
            errors.is_empty(),
            "well-formed reconstructed XML must not report: {errors:?}"
        );

        // Escaped content must survive the re-parse in find_divergent_fields
        // without triggering reconstruction errors (I-3 regression guard).
        let mod_c = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.c", "Mod C"),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><label>a &lt; b</label></ThingDef>"
                    .to_string(),
                source_file: "c.xml".to_string(),
            }],
        };
        let mod_d = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.d", "Mod D"),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><label>a &gt; b</label></ThingDef>"
                    .to_string(),
                source_file: "d.xml".to_string(),
            }],
        };
        let (conflicts, errors) = detect_replacements(&[mod_c, mod_d], None);
        assert!(
            errors.is_empty(),
            "escaped content must not error: {errors:?}"
        );
        assert_eq!(
            conflicts.len(),
            1,
            "genuinely divergent defs still conflict"
        );
    }

    #[test]
    fn cdata_only_divergence_is_detected_not_suppressed() {
        // Regression (FR-2c gate I-2): without a CData arm in
        // normalize_whitespace/extract_tag_content, labels differing only
        // inside CDATA canonicalized equal and all_identical suppressed the
        // conflict. raw_xml below is what def_parser emits after FR-2c.
        let mod_a = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml:
                    "<ThingDef><defName>Steel</defName><label><![CDATA[A]]></label></ThingDef>"
                        .to_string(),
                source_file: "a.xml".to_string(),
            }],
        };
        let mod_b = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.b", "Mod B"),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml:
                    "<ThingDef><defName>Steel</defName><label><![CDATA[B]]></label></ThingDef>"
                        .to_string(),
                source_file: "b.xml".to_string(),
            }],
        };
        let (conflicts, errors) = detect_replacements(&[mod_a, mod_b], None);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            conflicts.len(),
            1,
            "CDATA-only divergence must surface as a conflict"
        );
    }

    #[test]
    fn reconstruction_error_message_is_pinned() {
        // Two DISTINCT package ids so the same-def entries actually reach
        // find_divergent_fields (identical ids or identical XML short-circuit).
        let mod_a = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><label>a</label></ThingX>".to_string(),
                source_file: "a.xml".to_string(),
            }],
        };
        let mod_b = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.b", "Mod B"),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><label>b</label></ThingDef>"
                    .to_string(),
                source_file: "b.xml".to_string(),
            }],
        };
        let (_conflicts, errors) = detect_replacements(&[mod_a, mod_b], None);
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("internal XML reconstruction error for ThingDef:Steel: ")),
            "expected pinned reconstruction message, got: {errors:?}"
        );
    }

    #[test]
    fn detect_direct_replacement() {
        let mod_a = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>100</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.b", "Mod B"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>200</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let (conflicts, errors) = detect_replacements(&[mod_a, mod_b], None);
        assert_eq!(conflicts.len(), 1);
        assert!(errors.is_empty());

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
            mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: xml.to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.b", "Mod B"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: xml.to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let (conflicts, _errors) = detect_replacements(&[mod_a, mod_b], None);
        assert_eq!(
            conflicts.len(),
            0,
            "Identical defs should not produce a conflict"
        );
    }

    #[test]
    fn replacement_with_load_order_reports_winner() {
        let mod_a = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>100</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.b", "Mod B"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><statBases><MaxHitPoints>200</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let load_order = LoadOrder::new(vec!["test.mod.a".to_string(), "test.mod.b".to_string()]);

        let (conflicts, _errors) = detect_replacements(&[mod_a, mod_b], Some(&load_order));
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
            mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: xml_a.to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.b", "Mod B"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: xml_b.to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let (conflicts, _errors) = detect_replacements(&[mod_a, mod_b], None);
        assert_eq!(
            conflicts.len(),
            0,
            "Whitespace-only differences should not produce a conflict"
        );
    }

    #[test]
    fn divergent_fields_detected() {
        let mod_a = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><label>steel</label><statBases><MaxHitPoints>100</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_a/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let mod_b = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.b", "Mod B"),

            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: "<ThingDef><defName>Steel</defName><label>reinforced steel</label><statBases><MaxHitPoints>200</MaxHitPoints></statBases></ThingDef>".to_string(),
                source_file: "mod_b/Defs/ThingDefs.xml".to_string(),
            }],
        };

        let (conflicts, _errors) = detect_replacements(&[mod_a, mod_b], None);
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

    #[test]
    fn root_attribute_only_differences_are_reported() {
        let make = |package: &str, name: &str, nested: &str| ModDefs {
            mod_summary: test_mod_summary_named(package, package),
            defs: vec![ParsedDef {
                def_type: "ThingDef".to_string(),
                def_name: "Steel".to_string(),
                raw_xml: format!(
                    "<ThingDef Name=\"{name}\"><defName>Steel</defName><costList>{nested}</costList></ThingDef>"
                ),
                source_file: format!("{package}.xml"),
            }],
        };
        let (conflicts, errors) = detect_replacements(
            &[
                make("test.mod.a", "base", "<li A=\"1\" B=\"2\">x</li>"),
                make("test.mod.b", "other", "<li B=\"2\" A=\"1\">x</li>"),
            ],
            None,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let Conflict::Def(conflict) = &conflicts[0] else {
            panic!("expected def conflict")
        };
        let DefConflictMechanism::DirectReplacement { divergent_fields } = &conflict.mechanism
        else {
            panic!("expected direct replacement")
        };
        assert_eq!(divergent_fields, &["@Name"]);
    }

    #[test]
    fn multiple_differing_root_attributes_have_a_fixed_order() {
        // Qwen review I-3: `extract_root_attributes` returned a HashMap, whose
        // iteration order is randomized per process instance, so two differing
        // root attributes reached `divergent_fields` in either order. Repeated
        // calls discriminate reliably; a single call could pass by luck.
        let make = |package: &str, name: &str, parent: &str| ModDefs {
            mod_summary: test_mod_summary_named(package, package),
            defs: vec![ParsedDef {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
                raw_xml: format!(
                    "<ThingDef Name=\"{name}\" ParentName=\"{parent}\"><defName>Steel</defName></ThingDef>"
                ),
                source_file: format!("{package}.xml"),
            }],
        };
        let mods = [
            make("test.mod.a", "X", "BaseA"),
            make("test.mod.b", "Y", "BaseB"),
        ];
        let mut observed = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let (conflicts, errors) = detect_replacements(&mods, None);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(conflicts.len(), 1, "{conflicts:?}");
            let Conflict::Def(conflict) = &conflicts[0] else {
                panic!("expected def conflict")
            };
            let DefConflictMechanism::DirectReplacement { divergent_fields } = &conflict.mechanism
            else {
                panic!("expected direct replacement")
            };
            observed.insert(divergent_fields.clone());
        }
        assert_eq!(
            observed
                .into_iter()
                .map(|fields| fields.join(","))
                .collect::<Vec<_>>(),
            vec!["@Name,@ParentName"],
            "root attributes must be reported once, in sorted order"
        );
    }

    #[test]
    fn divergent_root_attributes_are_reported_in_canonical_union_order() {
        // Complements the repeated-call test: attributes held by only one side
        // must still land in sorted order, not base-then-other chain order.
        let make = |package: &str, xml: &str| ModDefs {
            mod_summary: test_mod_summary_named(package, package),
            defs: vec![ParsedDef {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
                raw_xml: xml.into(),
                source_file: format!("{package}.xml"),
            }],
        };
        let (conflicts, errors) = detect_replacements(
            &[
                make(
                    "test.mod.a",
                    "<ThingDef Zeta=\"1\"><defName>Steel</defName></ThingDef>",
                ),
                make(
                    "test.mod.b",
                    "<ThingDef Alpha=\"2\"><defName>Steel</defName></ThingDef>",
                ),
            ],
            None,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let Conflict::Def(conflict) = &conflicts[0] else {
            panic!("expected def conflict")
        };
        let DefConflictMechanism::DirectReplacement { divergent_fields } = &conflict.mechanism
        else {
            panic!("expected direct replacement")
        };
        assert_eq!(divergent_fields, &["@Alpha", "@Zeta"]);
    }

    #[test]
    fn root_and_nested_attribute_order_is_semantically_irrelevant() {
        let make = |package: &str, xml: &str| ModDefs {
            mod_summary: test_mod_summary_named(package, package),
            defs: vec![ParsedDef {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
                raw_xml: xml.into(),
                source_file: package.into(),
            }],
        };
        let a = make(
            "test.mod.a",
            "<ThingDef B=\"2\" A=\"1\"><defName>Steel</defName><node Z=\"9\" Y=\"8\"/></ThingDef>",
        );
        let b = make(
            "test.mod.b",
            "<ThingDef A=\"1\" B=\"2\"><defName>Steel</defName><node Y=\"8\" Z=\"9\"/></ThingDef>",
        );
        let (conflicts, errors) = detect_replacements(&[a, b], None);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            conflicts.is_empty(),
            "attribute order alone is not semantic: {conflicts:?}"
        );
    }

    #[test]
    fn equivalent_attribute_entities_are_canonical_and_invalid_attributes_are_diagnostic() {
        let make = |package: &str, xml: &str| ModDefs {
            mod_summary: test_mod_summary_named(package, package),
            defs: vec![ParsedDef {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
                raw_xml: xml.into(),
                source_file: package.into(),
            }],
        };
        let (conflicts, errors) = detect_replacements(
            &[
                make(
                    "test.mod.a",
                    "<ThingDef Root=\"A&amp;B\" Other=\"x\"><defName>Steel</defName><node Value=\"A&amp;B\" Other=\"x\"/><empty Value=\"A&amp;B\"/></ThingDef>",
                ),
                make(
                    "test.mod.b",
                    "<ThingDef Other=\"x\" Root=\"A&#38;B\"><defName>Steel</defName><node Other=\"x\" Value=\"A&#38;B\"/><empty Value=\"A&#38;B\"/></ThingDef>",
                ),
            ],
            None,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert!(conflicts.is_empty(), "equivalent attributes: {conflicts:?}");

        let (conflicts, errors) = detect_replacements(
            &[
                make(
                    "test.mod.c",
                    "<ThingDef Root=\"&unknown;\"><defName>Steel</defName></ThingDef>",
                ),
                make(
                    "test.mod.d",
                    "<ThingDef Root=\"&unknown;\"><defName>Steel</defName></ThingDef>",
                ),
            ],
            None,
        );
        assert!(
            !errors.is_empty(),
            "invalid attribute entity must be visible"
        );
        assert_eq!(
            conflicts.len(),
            1,
            "invalid attributes cannot establish equivalence"
        );
    }

    #[test]
    fn invalid_text_decoding_is_diagnostic_and_not_equivalent() {
        let make = |package: &str, text: &str| ModDefs {
            mod_summary: test_mod_summary_named(package, package),
            defs: vec![ParsedDef {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
                raw_xml: format!(
                    "<ThingDef><defName>Steel</defName><label>{text}</label></ThingDef>"
                ),
                source_file: package.into(),
            }],
        };
        let (conflicts, errors) = detect_replacements(
            &[
                make("test.mod.a", "&unknown;"),
                make("test.mod.b", "distinct"),
            ],
            None,
        );
        assert!(
            !errors.is_empty(),
            "invalid entity decoding must be visible"
        );
        assert_eq!(
            conflicts.len(),
            1,
            "bad text must not collapse into a false equivalent"
        );

        let (identical_invalid_conflicts, identical_invalid_errors) = detect_replacements(
            &[
                make("test.mod.c", "&unknown;"),
                make("test.mod.d", "&unknown;"),
            ],
            None,
        );
        assert!(!identical_invalid_errors.is_empty());
        assert_eq!(
            identical_invalid_conflicts.len(),
            1,
            "malformed text cannot establish equivalence"
        );
    }

    #[test]
    fn same_mod_version_folders_no_conflict() {
        let mod_defs = ModDefs {
            mod_summary: test_mod_summary_named("test.mod.versioned", "Versioned Mod"),

            defs: vec![
                ParsedDef {
                    def_type: "ThingDef".to_string(),
                    def_name: "VersionedItem".to_string(),
                    raw_xml: "<ThingDef><defName>VersionedItem</defName></ThingDef>".to_string(),
                    source_file: "mod_versioned/1.5/Defs/ThingDefs.xml".to_string(),
                },
                ParsedDef {
                    def_type: "ThingDef".to_string(),
                    def_name: "VersionedItem".to_string(),
                    raw_xml: "<ThingDef><defName>VersionedItem</defName><label>versioned item (legacy)</label></ThingDef>".to_string(),
                    source_file: "mod_versioned/Common/Defs/ThingDefs.xml".to_string(),
                },
            ],
        };

        let (conflicts, _errors) = detect_replacements(&[mod_defs], None);
        assert_eq!(
            conflicts.len(),
            0,
            "Same mod providing a def from multiple folders should not produce a conflict"
        );
    }
}
