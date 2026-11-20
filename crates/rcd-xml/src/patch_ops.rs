// PatchOperation parsing for RimWorld mod patches
//
// Parses `Patches/*.xml` files to extract PatchOperation elements, handling:
// - PatchOperationReplace, PatchOperationAdd, PatchOperationRemove, PatchOperationInsert
// - PatchOperationSequence (recursive extraction of nested operations)
// - PatchOperationFindMod (conditional operations gated on mod presence)
// - PatchOperationConditional (XPath test with match/nomatch branches)

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use rcd_core::content_files::collect_files_with_precedence_structured;
use rcd_core::xml_io::read_xml_file;
use std::path::{Path, PathBuf};

use crate::def_parser::check_well_formed_structured;

const MAX_OPERATION_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOpType {
    Replace,
    Add,
    Remove,
    Insert,
}

#[derive(Debug, Clone)]
pub struct PatchCondition {
    pub required_mod: String,
}

#[derive(Debug, Clone)]
pub struct ParsedPatchOp {
    pub op_type: PatchOpType,
    pub xpath: String,
    pub source_file: String,
    pub condition: Option<PatchCondition>,
}

/// Recursively walk a directory and parse all `.xml` files for patch operations.
pub fn parse_patches_dir(dir: &Path) -> (Vec<ParsedPatchOp>, Vec<String>) {
    parse_patches_dirs(&[dir.to_path_buf()])
}

/// Parse XML patch files from high-priority-first directories. The first
/// directory wins for each case-sensitive relative path.
pub fn parse_patches_dirs(dirs: &[PathBuf]) -> (Vec<ParsedPatchOp>, Vec<String>) {
    let (ops, errors) = parse_patches_dirs_structured(dirs);
    (ops, errors.into_iter().map(|issue| issue.message).collect())
}

pub(crate) fn parse_patches_dirs_structured(
    dirs: &[PathBuf],
) -> (Vec<ParsedPatchOp>, Vec<crate::XmlIssue>) {
    let (files, enumeration_errors) = collect_files_with_precedence_structured(dirs);
    let mut errors: Vec<crate::XmlIssue> = enumeration_errors.into_iter().map(Into::into).collect();
    let mut ops = Vec::new();
    for path in files {
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
        {
            continue;
        }
        match read_xml_file(&path) {
            Ok(contents) => {
                let source = path.to_string_lossy().to_string();
                let (parsed, xml_errors) = parse_patches_from_xml_structured(&contents, &source);
                ops.extend(parsed);
                errors.extend(xml_errors);
            }
            Err(error) => errors.push(crate::XmlIssue {
                message: format!("Failed to read {}: {}", path.display(), error),
                code: Some("xml_read_failure".into()),
                params: [
                    ("path".into(), path.display().to_string()),
                    ("detail".into(), error.to_string()),
                ]
                .into_iter()
                .collect(),
            }),
        }
    }
    (ops, errors)
}

/// Parse a single patches XML file and extract all patch operations.
///
/// Handles top-level `<Patch>` containing `<Operation Class="...">` elements.
pub fn parse_patches_from_xml(xml: &str, source: &str) -> (Vec<ParsedPatchOp>, Vec<String>) {
    let (ops, errors) = parse_patches_from_xml_structured(xml, source);
    (ops, errors.into_iter().map(|issue| issue.message).collect())
}

fn parse_patches_from_xml_structured(
    xml: &str,
    source: &str,
) -> (Vec<ParsedPatchOp>, Vec<crate::XmlIssue>) {
    let mut ops = Vec::new();
    let mut errors: Vec<crate::XmlIssue> = Vec::new();
    if let Some(msg) = check_well_formed_structured(xml, source) {
        errors.push(msg);
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut inside_patch = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if tag == "Patch" {
                    inside_patch = true;
                    continue;
                }

                if inside_patch && tag == "Operation" {
                    let class = extract_class_attr(e);
                    if let Some(class) = class {
                        parse_operation(
                            &mut reader,
                            &class,
                            source,
                            None,
                            1,
                            &mut ops,
                            &mut errors,
                        );
                    } else {
                        // Skip unknown operation without Class
                        skip_current_element_by_depth(&mut reader, source, &mut errors);
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "Patch" {
                    inside_patch = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(crate::XmlIssue::parse_error(source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    // FR-7a M-2: pre-check and structural capture report the same root error
    // byte-identically; dedup collapses the guaranteed pair. See def_parser.
    errors.sort_by(|a, b| a.message.cmp(&b.message));
    errors.dedup_by(|a, b| a.message == b.message);

    (ops, errors)
}

/// Parse a single `<Operation>` (or `<match>`) element given its Class attribute.
/// Recursively handles Sequence and FindMod.
fn parse_operation(
    reader: &mut Reader<&[u8]>,
    class: &str,
    source: &str,
    condition: Option<PatchCondition>,
    operation_depth: usize,
    ops: &mut Vec<ParsedPatchOp>,
    errors: &mut Vec<crate::XmlIssue>,
) {
    if operation_depth > MAX_OPERATION_DEPTH {
        errors.push(crate::XmlIssue::new(format!("Patch operation nesting exceeds maximum depth {MAX_OPERATION_DEPTH} in {source}; skipping operation"), "xml_patch_depth_exceeded", &[("path", source.into()), ("depth", MAX_OPERATION_DEPTH.to_string())]));
        skip_current_element_by_depth(reader, source, errors);
        return;
    }
    match class {
        "PatchOperationReplace"
        | "PatchOperationAdd"
        | "PatchOperationRemove"
        | "PatchOperationInsert" => {
            let op_type = match class {
                "PatchOperationReplace" => PatchOpType::Replace,
                "PatchOperationAdd" => PatchOpType::Add,
                "PatchOperationRemove" => PatchOpType::Remove,
                "PatchOperationInsert" => PatchOpType::Insert,
                _ => unreachable!(),
            };
            if let Some(xpath) = read_xpath_from_operation(reader, source, errors) {
                ops.push(ParsedPatchOp {
                    op_type,
                    xpath,
                    source_file: source.to_string(),
                    condition,
                });
            }
        }
        "PatchOperationSequence" => {
            // `PatchOperationSequence.success` runtime behavior is not evaluated;
            // conservatively assume all contained operations may execute.
            parse_sequence(reader, source, condition, operation_depth, ops, errors);
        }
        "PatchOperationFindMod" => {
            parse_find_mod(reader, source, operation_depth, ops, errors);
        }
        "PatchOperationConditional" => {
            parse_conditional(reader, source, condition, operation_depth, ops, errors);
        }
        _ => {
            // Unknown operation type — skip it. We don't know the enclosing
            // tag name, but we track depth to find the matching end tag.
            skip_current_element_by_depth(reader, source, errors);
        }
    }
}

/// Read the `<xpath>` child element value from within a simple patch operation
/// (Replace, Add, Remove, Insert). Skips remaining children and consumes
/// through the closing tag of the operation's enclosing element.
fn read_xpath_from_operation(
    reader: &mut Reader<&[u8]>,
    source: &str,
    errors: &mut Vec<crate::XmlIssue>,
) -> Option<String> {
    let mut xpath: Option<String> = None;
    let mut depth: u32 = 1; // We're inside the Operation/match element
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if depth == 2 && tag == "xpath" {
                    // reader-level trim_text does not reach CDATA, so trim here
                    // exactly like the <li> text list does.
                    xpath = read_element_text(reader, "xpath", source, errors)
                        .map(|text| text.trim().to_string());
                    depth -= 1;
                }
            }
            Ok(Event::Empty(ref e)) if e.name().as_ref() == b"xpath" && depth == 1 => {
                xpath = Some(String::new());
            }
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(crate::XmlIssue::parse_error(source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    xpath
}

/// Parse `PatchOperationSequence`: contains `<operations><li>` elements that
/// are themselves operations.
fn parse_sequence(
    reader: &mut Reader<&[u8]>,
    source: &str,
    condition: Option<PatchCondition>,
    operation_depth: usize,
    ops: &mut Vec<ParsedPatchOp>,
    errors: &mut Vec<crate::XmlIssue>,
) {
    let mut depth: u32 = 1;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                // <li Class="PatchOperationReplace"> inside <operations>
                if tag == "li"
                    && let Some(class) = extract_class_attr(e)
                {
                    parse_operation(
                        reader,
                        &class,
                        source,
                        condition.clone(),
                        operation_depth + 1,
                        ops,
                        errors,
                    );
                    // parse_operation consumed through the closing </li>, so adjust depth
                    depth -= 1;
                }
            }
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(crate::XmlIssue::parse_error(source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }
}

/// Parse `PatchOperationFindMod`: extract mod names from `<mods><li>` and
/// parse `<match>` as a nested operation with the condition applied.
fn parse_find_mod(
    reader: &mut Reader<&[u8]>,
    source: &str,
    operation_depth: usize,
    ops: &mut Vec<ParsedPatchOp>,
    errors: &mut Vec<crate::XmlIssue>,
) {
    let mut depth: u32 = 1;
    let mut buf = Vec::new();
    let mut required_mods: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if tag == "mods" {
                    // Parse <mods><li>ModName</li></mods>
                    required_mods = read_li_text_list(reader, source, errors);
                    depth -= 1; // read_li_text_list consumes through </mods>
                } else if tag == "match" {
                    // <match Class="PatchOperationReplace">
                    if let Some(class) = extract_class_attr(e) {
                        // Create condition from first mod name (primary)
                        let condition = required_mods.first().map(|m| PatchCondition {
                            required_mod: m.clone(),
                        });
                        parse_operation(
                            reader,
                            &class,
                            source,
                            condition,
                            operation_depth + 1,
                            ops,
                            errors,
                        );
                        depth -= 1; // parse_operation consumed through </match>
                    }
                }
            }
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(crate::XmlIssue::parse_error(source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }
}

/// Parse `PatchOperationConditional`: has an `<xpath>` test, and `<match>` / `<nomatch>`
/// child operations. For static analysis we cannot evaluate the test, so we extract
/// operations from both branches.
fn parse_conditional(
    reader: &mut Reader<&[u8]>,
    source: &str,
    condition: Option<PatchCondition>,
    operation_depth: usize,
    ops: &mut Vec<ParsedPatchOp>,
    errors: &mut Vec<crate::XmlIssue>,
) {
    let mut depth: u32 = 1;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if depth == 2 && (tag == "match" || tag == "nomatch") {
                    if let Some(class) = extract_class_attr(e) {
                        parse_operation(
                            reader,
                            &class,
                            source,
                            condition.clone(),
                            operation_depth + 1,
                            ops,
                            errors,
                        );
                        depth -= 1;
                    }
                } else if depth == 2 && tag == "xpath" {
                    // Skip the test xpath — we don't evaluate it
                    skip_current_element_by_depth(reader, source, errors);
                    depth -= 1;
                }
            }
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(crate::XmlIssue::parse_error(source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }
}

/// Read a list of `<li>text</li>` elements from the current element (e.g., `<mods>`).
/// Consumes through the closing tag of the parent element.
fn read_li_text_list(
    reader: &mut Reader<&[u8]>,
    source: &str,
    errors: &mut Vec<crate::XmlIssue>,
) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth: u32 = 1;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "li" && depth == 2 {
                    if let Some(text) = read_element_text(reader, "li", source, errors) {
                        items.push(text.trim().to_string());
                    }
                    depth -= 1;
                }
            }
            Ok(Event::Empty(ref e)) if e.name().as_ref() == b"li" && depth == 1 => {
                items.push(String::new());
            }
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(crate::XmlIssue::parse_error(source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    items
}

/// Read a text-only element, consuming through its matching end tag. Comments are
/// ignored and both escaped text and CDATA are accepted. Nested elements are
/// malformed for these scalar fields, but are consumed to preserve the caller's
/// position at the next sibling.
fn read_element_text(
    reader: &mut Reader<&[u8]>,
    element: &str,
    source: &str,
    errors: &mut Vec<crate::XmlIssue>,
) -> Option<String> {
    let mut text = String::new();
    let mut nested: Vec<String> = Vec::new();
    let mut malformed = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(value)) if nested.is_empty() => match value.unescape() {
                Ok(value) => text.push_str(&value),
                Err(error) => {
                    if !malformed {
                        errors.push(crate::XmlIssue::new(
                            format!("Malformed <{element}> text in {source}: {error}"),
                            "xml_text_decode_failure",
                            &[
                                ("path", source.into()),
                                ("element", element.into()),
                                ("detail", error.to_string()),
                            ],
                        ));
                    }
                    malformed = true;
                }
            },
            Ok(Event::CData(value)) if nested.is_empty() => match value.decode() {
                Ok(value) => text.push_str(&value),
                Err(error) => {
                    if !malformed {
                        errors.push(crate::XmlIssue::new(
                            format!("Malformed <{element}> text in {source}: {error}"),
                            "xml_text_decode_failure",
                            &[
                                ("path", source.into()),
                                ("element", element.into()),
                                ("detail", error.to_string()),
                            ],
                        ));
                    }
                    malformed = true;
                }
            },
            Ok(Event::Start(value)) => {
                if !malformed {
                    errors.push(crate::XmlIssue::new(
                        format!("Nested element in <{element}> text in {source}"),
                        "xml_text_nested",
                        &[("path", source.into()), ("element", element.into())],
                    ));
                }
                malformed = true;
                nested.push(String::from_utf8_lossy(value.name().as_ref()).into_owned());
            }
            Ok(Event::Empty(_)) => {
                if !malformed {
                    errors.push(crate::XmlIssue::new(
                        format!("Nested element in <{element}> text in {source}"),
                        "xml_text_nested",
                        &[("path", source.into()), ("element", element.into())],
                    ));
                }
                malformed = true;
            }
            Ok(Event::End(value)) => {
                let name = String::from_utf8_lossy(value.name().as_ref()).into_owned();
                if name == element {
                    if !nested.is_empty() && !malformed {
                        errors.push(crate::XmlIssue::new(
                            format!("Mismatched element in <{element}> text in {source}"),
                            "xml_text_mismatched",
                            &[("path", source.into()), ("element", element.into())],
                        ));
                        malformed = true;
                    }
                    return (!malformed).then_some(text);
                }
                if let Some(expected) = nested.pop() {
                    if name != expected && !malformed {
                        errors.push(crate::XmlIssue::new(
                            format!("Mismatched element in <{element}> text in {source}"),
                            "xml_text_mismatched",
                            &[("path", source.into()), ("element", element.into())],
                        ));
                        malformed = true;
                    }
                } else {
                    if !malformed {
                        errors.push(crate::XmlIssue::new(
                            format!("Mismatched end element in <{element}> text in {source}"),
                            "xml_text_mismatched_end",
                            &[("path", source.into()), ("element", element.into())],
                        ));
                    }
                    return None;
                }
            }
            Ok(Event::Eof) => {
                errors.push(crate::XmlIssue::new(
                    format!("Unexpected EOF in <{element}> text in {source}"),
                    "xml_text_unexpected_eof",
                    &[("path", source.into()), ("element", element.into())],
                ));
                return None;
            }
            Err(error) => {
                errors.push(crate::XmlIssue::parse_error(source, error));
                return None;
            }
            _ => {}
        }
        buf.clear();
    }
}

/// Extract the `Class` attribute from an element.
fn extract_class_attr(e: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    for attr in e.attributes().filter_map(|a| a.ok()) {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        if key == "Class" {
            return Some(String::from_utf8_lossy(&attr.value).to_string());
        }
    }
    None
}

/// Skip the current element by tracking depth.
fn skip_current_element_by_depth(
    reader: &mut Reader<&[u8]>,
    source: &str,
    errors: &mut Vec<crate::XmlIssue>,
) {
    let mut depth: u32 = 1;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(crate::XmlIssue::parse_error(source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mod_c_patches() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/mods/mod_c/Patches");

        let (ops, _errors) = parse_patches_dir(&fixtures);
        assert_eq!(ops.len(), 2, "mod_c should have 2 patch operations");

        assert_eq!(ops[0].op_type, PatchOpType::Replace);
        assert_eq!(
            ops[0].xpath,
            "Defs/ThingDef[defName=\"Steel\"]/statBases/MaxHitPoints"
        );
        assert!(ops[0].condition.is_none());

        assert_eq!(ops[1].op_type, PatchOpType::Add);
        assert_eq!(ops[1].xpath, "Defs/ThingDef[defName=\"Steel\"]/statBases");
        assert!(ops[1].condition.is_none());
    }

    #[test]
    fn parse_mod_d_conditional_patch() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/mods/mod_d/Patches");

        let (ops, _errors) = parse_patches_dir(&fixtures);
        assert_eq!(ops.len(), 1, "mod_d should have 1 patch operation");

        assert_eq!(ops[0].op_type, PatchOpType::Replace);
        assert_eq!(
            ops[0].xpath,
            "Defs/ThingDef[defName=\"Steel\"]/statBases/MaxHitPoints"
        );

        let condition = ops[0].condition.as_ref().expect("should have a condition");
        assert_eq!(condition.required_mod, "Test Mod C");
    }

    #[test]
    fn parse_simple_replace() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationReplace">
        <xpath>Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints</xpath>
        <value><MaxHitPoints>150</MaxHitPoints></value>
    </Operation>
</Patch>"#;

        let (ops, errors) = parse_patches_from_xml(xml, "test.xml");
        assert!(errors.is_empty());
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].op_type, PatchOpType::Replace);
        assert_eq!(
            ops[0].xpath,
            r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#
        );
        assert_eq!(ops[0].source_file, "test.xml");
    }

    #[test]
    fn parse_sequence() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationSequence">
        <operations>
            <li Class="PatchOperationReplace">
                <xpath>Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints</xpath>
                <value><MaxHitPoints>150</MaxHitPoints></value>
            </li>
            <li Class="PatchOperationAdd">
                <xpath>Defs/ThingDef[defName="Steel"]/costList</xpath>
                <value><Plasteel>5</Plasteel></value>
            </li>
        </operations>
    </Operation>
</Patch>"#;

        let (ops, _errors) = parse_patches_from_xml(xml, "test.xml");
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].op_type, PatchOpType::Replace);
        assert_eq!(ops[1].op_type, PatchOpType::Add);
    }

    #[test]
    fn parse_find_mod() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationFindMod">
        <mods>
            <li>Some Required Mod</li>
        </mods>
        <match Class="PatchOperationReplace">
            <xpath>Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints</xpath>
            <value><MaxHitPoints>175</MaxHitPoints></value>
        </match>
    </Operation>
</Patch>"#;

        let (ops, _errors) = parse_patches_from_xml(xml, "test.xml");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].op_type, PatchOpType::Replace);
        assert!(ops[0].condition.is_some());
        assert_eq!(
            ops[0].condition.as_ref().unwrap().required_mod,
            "Some Required Mod"
        );
    }

    #[test]
    fn parse_remove_and_insert() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationRemove">
        <xpath>Defs/ThingDef[defName="Steel"]/costList</xpath>
    </Operation>
    <Operation Class="PatchOperationInsert">
        <xpath>Defs/ThingDef[defName="Steel"]/statBases</xpath>
        <value><Mass>0.5</Mass></value>
    </Operation>
</Patch>"#;

        let (ops, _errors) = parse_patches_from_xml(xml, "test.xml");
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].op_type, PatchOpType::Remove);
        assert_eq!(ops[1].op_type, PatchOpType::Insert);
    }

    #[test]
    fn unknown_operation_skipped() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationTest">
        <xpath>Defs/ThingDef[defName="Steel"]/something</xpath>
        <success Class="PatchOperationReplace">
            <xpath>Defs/ThingDef[defName="Steel"]/statBases</xpath>
            <value><MaxHitPoints>999</MaxHitPoints></value>
        </success>
    </Operation>
    <Operation Class="PatchOperationReplace">
        <xpath>Defs/ThingDef[defName="Wood"]/statBases/MaxHitPoints</xpath>
        <value><MaxHitPoints>50</MaxHitPoints></value>
    </Operation>
</Patch>"#;

        let (ops, _errors) = parse_patches_from_xml(xml, "test.xml");
        // Should skip the unknown PatchOperationTest and only pick up the Replace
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].op_type, PatchOpType::Replace);
        assert!(ops[0].xpath.contains("Wood"));
    }

    #[test]
    fn parse_conditional_match_only() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationConditional">
        <xpath>Defs/ThingDef[defName="Steel"]/comps</xpath>
        <match Class="PatchOperationAdd">
            <xpath>Defs/ThingDef[defName="Steel"]/comps</xpath>
            <value><li><compClass>MyComp</compClass></li></value>
        </match>
    </Operation>
</Patch>"#;

        let (ops, _errors) = parse_patches_from_xml(xml, "test.xml");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].op_type, PatchOpType::Add);
        assert_eq!(ops[0].xpath, r#"Defs/ThingDef[defName="Steel"]/comps"#);
        assert!(ops[0].condition.is_none());
    }

    #[test]
    fn parse_conditional_nomatch_only() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationConditional">
        <xpath>Defs/ThingDef[defName="Steel"]/comps</xpath>
        <nomatch Class="PatchOperationAdd">
            <xpath>Defs/ThingDef[defName="Steel"]</xpath>
            <value><comps><li><compClass>MyComp</compClass></li></comps></value>
        </nomatch>
    </Operation>
</Patch>"#;

        let (ops, _errors) = parse_patches_from_xml(xml, "test.xml");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].op_type, PatchOpType::Add);
        assert_eq!(ops[0].xpath, r#"Defs/ThingDef[defName="Steel"]"#);
        assert!(ops[0].condition.is_none());
    }

    #[test]
    fn parse_conditional_both_branches() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationConditional">
        <xpath>Defs/ThingDef[defName="Steel"]/comps</xpath>
        <match Class="PatchOperationAdd">
            <xpath>Defs/ThingDef[defName="Steel"]/comps</xpath>
            <value><li><compClass>MyComp</compClass></li></value>
        </match>
        <nomatch Class="PatchOperationAdd">
            <xpath>Defs/ThingDef[defName="Steel"]</xpath>
            <value><comps><li><compClass>MyComp</compClass></li></comps></value>
        </nomatch>
    </Operation>
</Patch>"#;

        let (ops, _errors) = parse_patches_from_xml(xml, "test.xml");
        assert_eq!(
            ops.len(),
            2,
            "Both match and nomatch should produce operations"
        );
        assert_eq!(ops[0].op_type, PatchOpType::Add);
        assert_eq!(ops[0].xpath, r#"Defs/ThingDef[defName="Steel"]/comps"#);
        assert_eq!(ops[1].op_type, PatchOpType::Add);
        assert_eq!(ops[1].xpath, r#"Defs/ThingDef[defName="Steel"]"#);
    }

    #[test]
    fn parse_conditional_with_nested_sequence() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Patch>
    <Operation Class="PatchOperationConditional">
        <xpath>Defs/ThingDef[defName="Steel"]/comps</xpath>
        <nomatch Class="PatchOperationSequence">
            <operations>
                <li Class="PatchOperationAdd">
                    <xpath>Defs/ThingDef[defName="Steel"]</xpath>
                    <value><comps /></value>
                </li>
                <li Class="PatchOperationAdd">
                    <xpath>Defs/ThingDef[defName="Steel"]/comps</xpath>
                    <value><li><compClass>MyComp</compClass></li></value>
                </li>
            </operations>
        </nomatch>
    </Operation>
</Patch>"#;

        let (ops, _errors) = parse_patches_from_xml(xml, "test.xml");
        assert_eq!(
            ops.len(),
            2,
            "Sequence inside nomatch should produce 2 operations"
        );
        assert_eq!(ops[0].op_type, PatchOpType::Add);
        assert_eq!(ops[1].op_type, PatchOpType::Add);
    }

    #[test]
    fn parse_mod_e_conditional_patch() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/mods/mod_e/Patches");

        let (ops, _errors) = parse_patches_dir(&fixtures);
        assert_eq!(
            ops.len(),
            2,
            "mod_e conditional should produce 2 operations (match + nomatch)"
        );
        assert_eq!(ops[0].op_type, PatchOpType::Add);
        assert_eq!(ops[1].op_type, PatchOpType::Add);
    }

    #[test]
    fn parse_patches_dirs_uses_first_directory_for_duplicate_relative_path() {
        let high = tempfile::tempdir().unwrap();
        let low = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(high.path().join("Patches")).unwrap();
        std::fs::create_dir_all(low.path().join("Patches")).unwrap();
        std::fs::write(high.path().join("Patches/shared.xml"), r#"<Patch><Operation Class="PatchOperationReplace"><xpath>Defs/High</xpath></Operation></Patch>"#).unwrap();
        std::fs::write(low.path().join("Patches/shared.xml"), r#"<Patch><Operation Class="PatchOperationReplace"><xpath>Defs/Low</xpath></Operation></Patch>"#).unwrap();

        let (ops, errors) =
            parse_patches_dirs(&[high.path().to_path_buf(), low.path().to_path_buf()]);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].xpath, "Defs/High");
    }

    #[test]
    fn parse_patches_dirs_reports_invalid_utf8() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("invalid.xml"), [0xff]).unwrap();
        let (ops, errors) = parse_patches_dirs(&[dir.path().to_path_buf()]);
        assert!(ops.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("invalid.xml"));
    }

    #[test]
    fn parse_patches_dir_returns_errors_vec() {
        let dir = tempfile::tempdir().unwrap();
        let patches_dir = dir.path().join("Patches");
        std::fs::create_dir_all(&patches_dir).unwrap();
        std::fs::write(
            patches_dir.join("Patch.xml"),
            r#"<Patch>
                <Operation Class="PatchOperationReplace">
                    <xpath>Defs/ThingDef[defName="Steel"]/statBases</xpath>
                    <value><statBases><MaxHitPoints>500</MaxHitPoints></statBases></value>
                </Operation>
            </Patch>"#,
        )
        .unwrap();

        let (ops, errors) = parse_patches_dir(patches_dir.as_path());
        assert_eq!(ops.len(), 1);
        assert!(errors.is_empty());
    }

    #[test]
    fn malformed_patches_xml_reports_error_and_keeps_partial_results() {
        // Truncate mid opening-tag: quick-xml is non-validating, so truncation
        // after well-formed text yields Ok(Eof); only a mid-tag cut produces Err.
        let xml = r#"<Patch>
    <Operation Class="PatchOperationReplace">
        <xpath>Defs/ThingDef[defName="Steel"]/label</xpath>
        <value><label>Iron</label></value>
    </Operation>
    <Operation Class="PatchOperationAdd"#;

        let (ops, errors) = parse_patches_from_xml(xml, "test.xml");
        assert!(
            ops.iter().any(|o| o.xpath.contains("Steel")),
            "valid op before the error should be kept"
        );
        assert!(!errors.is_empty(), "parse error must be reported");
        assert!(errors.iter().all(|e| e.contains("test.xml")));
    }

    #[test]
    fn xpath_text_reader_handles_empty_comments_cdata_and_preserves_siblings() {
        let xml = r#"<Patch>
            <Operation Class="PatchOperationReplace"><xpath></xpath></Operation>
            <Operation Class="PatchOperationReplace"><xpath><!-- before -->Defs/Comment</xpath></Operation>
            <Operation Class="PatchOperationReplace"><xpath><![CDATA[Defs/CData]]></xpath></Operation>
            <Operation Class="PatchOperationReplace"><xpath>Defs/Next</xpath></Operation>
        </Patch>"#;

        let (ops, errors) = parse_patches_from_xml(xml, "xpath-text.xml");
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(
            ops.iter().map(|op| op.xpath.as_str()).collect::<Vec<_>>(),
            ["", "Defs/Comment", "Defs/CData", "Defs/Next"]
        );
    }

    #[test]
    fn cdata_padding_in_xpath_is_trimmed_so_normalization_resolves() {
        // Qwen review M-2: quick-xml's trim_text does not apply to CDATA, so
        // `<xpath><![CDATA[ Defs/ThingDef ]]></xpath>` kept its padding and
        // normalize_xpath could not find the def type. Predicated CDATA text
        // happened to resolve, which is why this test pins both forms.
        use crate::xpath::normalize_xpath;
        let xml = r#"<Patch>
            <Operation Class="PatchOperationRemove"><xpath><![CDATA[ Defs/ThingDef ]]></xpath></Operation>
            <Operation Class="PatchOperationRemove"><xpath><![CDATA[ Defs/ThingDef[defName="Steel"] ]]></xpath></Operation>
            <Operation Class="PatchOperationRemove"><xpath><![CDATA[Defs/CData]]></xpath></Operation>
            <Operation Class="PatchOperationRemove"><xpath>Defs/Plain</xpath></Operation>
        </Patch>"#;

        let (ops, errors) = parse_patches_from_xml(xml, "cdata-xpath.xml");
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(
            ops.iter().map(|op| op.xpath.as_str()).collect::<Vec<_>>(),
            [
                "Defs/ThingDef",
                "Defs/ThingDef[defName=\"Steel\"]",
                "Defs/CData",
                "Defs/Plain"
            ],
            "parser output must match the trimmed <li> list path"
        );
        assert_eq!(
            normalize_xpath(&ops[0].xpath).def_type.as_deref(),
            Some("ThingDef")
        );
        assert_eq!(
            normalize_xpath(&ops[1].xpath).def_name.as_deref(),
            Some("Steel")
        );
    }

    #[test]
    fn malformed_nested_xpath_is_diagnosed_and_following_operation_survives() {
        let xml = r#"<Patch>
            <Operation Class="PatchOperationReplace"><xpath><nested>bad</xpath></Operation>
            <Operation Class="PatchOperationAdd"><xpath>Defs/Survivor</xpath></Operation>
        </Patch>"#;

        let (ops, errors) = parse_patches_from_xml(xml, "malformed-xpath.xml");
        assert!(
            !errors.is_empty(),
            "malformed xpath content needs a diagnostic"
        );
        assert_eq!(ops.len(), 1, "malformed operation should not be emitted");
        assert_eq!(ops[0].xpath, "Defs/Survivor");
    }

    #[test]
    fn li_text_reader_handles_empty_comments_cdata_for_find_mod_and_conditional() {
        let xml = r#"<Patch>
            <Operation Class="PatchOperationFindMod"><mods><li></li></mods><match Class="PatchOperationReplace"><xpath>Defs/Empty</xpath></match></Operation>
            <Operation Class="PatchOperationFindMod"><mods><li><!-- before -->Mod A</li></mods><match Class="PatchOperationReplace"><xpath>Defs/Comment</xpath></match></Operation>
            <Operation Class="PatchOperationFindMod"><mods><li><![CDATA[Mod B]]></li></mods><match Class="PatchOperationReplace"><xpath>Defs/CData</xpath></match></Operation>
            <Operation Class="PatchOperationConditional"><xpath>Defs/Test</xpath><match Class="PatchOperationAdd"><xpath>Defs/Conditional</xpath></match><nomatch Class="PatchOperationAdd"><xpath>Defs/NoMatch</xpath></nomatch></Operation>
            <Operation Class="PatchOperationReplace"><xpath>Defs/Following</xpath></Operation>
        </Patch>"#;

        let (ops, errors) = parse_patches_from_xml(xml, "li-text.xml");
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(ops.len(), 6);
        assert_eq!(ops[0].condition.as_ref().unwrap().required_mod, "");
        assert_eq!(ops[1].condition.as_ref().unwrap().required_mod, "Mod A");
        assert_eq!(ops[2].condition.as_ref().unwrap().required_mod, "Mod B");
        assert_eq!(ops[3].xpath, "Defs/Conditional");
        assert_eq!(ops[4].xpath, "Defs/NoMatch");
        assert_eq!(ops[5].xpath, "Defs/Following");
    }

    #[test]
    fn sequence_depth_overflow_skips_subtree_and_preserves_sibling() {
        let mut xml =
            String::from(r#"<Patch><Operation Class="PatchOperationSequence"><operations>"#);
        for _ in 0..65 {
            xml.push_str(r#"<li Class="PatchOperationSequence"><operations>"#);
        }
        xml.push_str(r#"<li Class="PatchOperationAdd"><xpath>Defs/TooDeep</xpath></li>"#);
        for _ in 0..65 {
            xml.push_str("</operations></li>");
        }
        xml.push_str(r#"</operations></Operation><Operation Class="PatchOperationReplace"><xpath>Defs/Survivor</xpath></Operation></Patch>"#);

        let (ops, errors) = parse_patches_from_xml(&xml, "deep-sequence.xml");
        let (_, structured) = parse_patches_from_xml_structured(&xml, "deep-sequence.xml");
        let depth = structured
            .iter()
            .find(|issue| issue.code.as_deref() == Some("xml_patch_depth_exceeded"))
            .unwrap();
        assert_eq!(depth.params["path"], "deep-sequence.xml");
        assert_eq!(depth.params["depth"], "64");
        assert!(errors.contains(&depth.message));
        assert!(!errors.is_empty(), "depth overflow needs a diagnostic");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].xpath, "Defs/Survivor");
    }

    #[test]
    fn malformed_xpath_text_exposes_external_detail_and_eof() {
        let xml = r#"<Patch><Operation Class="PatchOperationAdd"><xpath>&bogus;"#;
        let (_, issues) = parse_patches_from_xml_structured(xml, "bad.xml");
        let decode = issues
            .iter()
            .find(|issue| issue.code.as_deref() == Some("xml_text_decode_failure"))
            .unwrap();
        assert_eq!(decode.params["path"], "bad.xml");
        assert_eq!(decode.params["element"], "xpath");
        assert!(!decode.params["detail"].is_empty());
        let eof = issues
            .iter()
            .find(|issue| issue.code.as_deref() == Some("xml_text_unexpected_eof"))
            .unwrap();
        assert_eq!(eof.params["element"], "xpath");
        assert!(issues.iter().all(|issue| issue.code.is_some()));
    }

    #[test]
    fn unclosed_cdata_in_xpath_yields_diagnostic_via_precheck() {
        let xml = r#"<Patch><Operation Class="PatchOperationReplace"><xpath><![CDATA[Defs/ThingDef</xpath><value><label>x</label></value></Operation></Patch>"#;
        let (_ops, errors) = parse_patches_from_xml(xml, "t.xml");
        assert!(
            !errors.is_empty(),
            "pre-check must surface the malformed document"
        );
        assert!(errors.iter().all(|e| e.contains("t.xml")));
    }
}
