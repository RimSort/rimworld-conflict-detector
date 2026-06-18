// PatchOperation parsing for RimWorld mod patches
//
// Parses `Patches/*.xml` files to extract PatchOperation elements, handling:
// - PatchOperationReplace, PatchOperationAdd, PatchOperationRemove, PatchOperationInsert
// - PatchOperationSequence (recursive extraction of nested operations)
// - PatchOperationFindMod (conditional operations gated on mod presence)
// - PatchOperationConditional (XPath test with match/nomatch branches)

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::path::Path;
use walkdir::WalkDir;

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
pub fn parse_patches_dir(dir: &Path) -> Result<Vec<ParsedPatchOp>, Box<dyn std::error::Error>> {
    let mut ops = Vec::new();

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
        })
    {
        let path = entry.path();
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let source = path.to_string_lossy().to_string();
                ops.extend(parse_patches_from_xml(&contents, &source));
            }
            Err(_) => continue,
        }
    }

    Ok(ops)
}

/// Parse a single patches XML file and extract all patch operations.
///
/// Handles top-level `<Patch>` containing `<Operation Class="...">` elements.
pub fn parse_patches_from_xml(xml: &str, source: &str) -> Vec<ParsedPatchOp> {
    let mut ops = Vec::new();
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
                        parse_operation(&mut reader, &class, source, None, &mut ops);
                    } else {
                        // Skip unknown operation without Class
                        skip_current_element_by_depth(&mut reader);
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
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    ops
}

/// Parse a single `<Operation>` (or `<match>`) element given its Class attribute.
/// Recursively handles Sequence and FindMod.
fn parse_operation(
    reader: &mut Reader<&[u8]>,
    class: &str,
    source: &str,
    condition: Option<PatchCondition>,
    ops: &mut Vec<ParsedPatchOp>,
) {
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
            if let Some(xpath) = read_xpath_from_operation(reader) {
                ops.push(ParsedPatchOp {
                    op_type,
                    xpath,
                    source_file: source.to_string(),
                    condition,
                });
            }
        }
        "PatchOperationSequence" => {
            parse_sequence(reader, source, condition, ops);
        }
        "PatchOperationFindMod" => {
            parse_find_mod(reader, source, ops);
        }
        "PatchOperationConditional" => {
            parse_conditional(reader, source, condition, ops);
        }
        _ => {
            // Unknown operation type — skip it. We don't know the enclosing
            // tag name, but we track depth to find the matching end tag.
            skip_current_element_by_depth(reader);
        }
    }
}

/// Read the `<xpath>` child element value from within a simple patch operation
/// (Replace, Add, Remove, Insert). Skips remaining children and consumes
/// through the closing tag of the operation's enclosing element.
fn read_xpath_from_operation(reader: &mut Reader<&[u8]>) -> Option<String> {
    let mut xpath: Option<String> = None;
    let mut depth: u32 = 1; // We're inside the Operation/match element
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if depth == 2 && tag == "xpath" {
                    // Read the text content of <xpath>
                    let mut text_buf = Vec::new();
                    if let Ok(Event::Text(t)) = reader.read_event_into(&mut text_buf)
                        && let Ok(text) = t.unescape()
                    {
                        xpath = Some(text.trim().to_string());
                    }
                    // Consume closing </xpath>
                    let mut end_buf = Vec::new();
                    let _ = reader.read_event_into(&mut end_buf);
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
            Err(_) => break,
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
    ops: &mut Vec<ParsedPatchOp>,
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
                    parse_operation(reader, &class, source, condition.clone(), ops);
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
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// Parse `PatchOperationFindMod`: extract mod names from `<mods><li>` and
/// parse `<match>` as a nested operation with the condition applied.
fn parse_find_mod(reader: &mut Reader<&[u8]>, source: &str, ops: &mut Vec<ParsedPatchOp>) {
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
                    required_mods = read_li_text_list(reader);
                    depth -= 1; // read_li_text_list consumes through </mods>
                } else if tag == "match" {
                    // <match Class="PatchOperationReplace">
                    if let Some(class) = extract_class_attr(e) {
                        // Create condition from first mod name (primary)
                        let condition = required_mods.first().map(|m| PatchCondition {
                            required_mod: m.clone(),
                        });
                        parse_operation(reader, &class, source, condition, ops);
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
            Err(_) => break,
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
    ops: &mut Vec<ParsedPatchOp>,
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
                        parse_operation(reader, &class, source, condition.clone(), ops);
                        depth -= 1;
                    }
                } else if depth == 2 && tag == "xpath" {
                    // Skip the test xpath — we don't evaluate it
                    skip_current_element_by_depth(reader);
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
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// Read a list of `<li>text</li>` elements from the current element (e.g., `<mods>`).
/// Consumes through the closing tag of the parent element.
fn read_li_text_list(reader: &mut Reader<&[u8]>) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth: u32 = 1;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "li" && depth == 2 {
                    // Read the text
                    let mut text_buf = Vec::new();
                    if let Ok(Event::Text(t)) = reader.read_event_into(&mut text_buf)
                        && let Ok(text) = t.unescape()
                    {
                        items.push(text.trim().to_string());
                    }
                    // Consume </li>
                    let mut end_buf = Vec::new();
                    let _ = reader.read_event_into(&mut end_buf);
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
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    items
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
fn skip_current_element_by_depth(reader: &mut Reader<&[u8]>) {
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
            Err(_) => break,
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

        let ops = parse_patches_dir(&fixtures).expect("should parse patches dir");
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

        let ops = parse_patches_dir(&fixtures).expect("should parse patches dir");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_from_xml(xml, "test.xml");
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

        let ops = parse_patches_dir(&fixtures).expect("should parse patches dir");
        assert_eq!(
            ops.len(),
            2,
            "mod_e conditional should produce 2 operations (match + nomatch)"
        );
        assert_eq!(ops[0].op_type, PatchOpType::Add);
        assert_eq!(ops[1].op_type, PatchOpType::Add);
    }
}
