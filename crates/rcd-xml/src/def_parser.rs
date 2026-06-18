use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::path::Path;
use walkdir::WalkDir;

/// A single parsed def extracted from an XML file.
#[derive(Debug, Clone)]
pub struct ParsedDef {
    pub def_type: String,
    pub def_name: String,
    pub raw_xml: String,
    pub source_file: String,
}

/// Recursively walks a directory, parses all `.xml` files, and returns all defs found.
///
/// Returns a tuple of (parsed defs, count of abstract defs without defName that were skipped, I/O errors).
pub fn parse_defs_dir(dir: &Path) -> (Vec<ParsedDef>, usize, Vec<String>) {
    let mut defs = Vec::new();
    let mut total_skipped: usize = 0;
    let mut errors = Vec::new();

    for result in WalkDir::new(dir).into_iter() {
        let entry = match result {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("Failed to read directory entry: {}", e));
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }
        if !entry
            .path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
        {
            continue;
        }

        let path = entry.path();
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let source = path.to_string_lossy().to_string();
                let (parsed, skipped) = parse_defs_from_xml(&contents, &source);
                defs.extend(parsed);
                total_skipped += skipped;
            }
            Err(e) => {
                errors.push(format!("Failed to read {}: {}", path.display(), e));
            }
        }
    }

    (defs, total_skipped, errors)
}

/// Parses XML content for defs. Finds top-level elements under `<Defs>` whose tag names
/// end with "Def" (e.g., `ThingDef`, `RecipeDef`). Extracts the `defName` child and
/// captures the raw XML of the entire def element.
///
/// Abstract defs (those with `Abstract="True"`) that lack a `defName` are skipped.
///
/// Returns a tuple of (parsed defs, count of abstract defs without defName that were skipped).
pub fn parse_defs_from_xml(xml: &str, source: &str) -> (Vec<ParsedDef>, usize) {
    let mut defs = Vec::new();
    let mut abstract_skipped: usize = 0;
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut inside_defs = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if tag_name == "Defs" {
                    inside_defs = true;
                    continue;
                }

                if inside_defs && tag_name.ends_with("Def") {
                    // Check for Abstract="True" attribute
                    let is_abstract = e.attributes().filter_map(|a| a.ok()).any(|a| {
                        let key = String::from_utf8_lossy(a.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&a.value).to_string();
                        key == "Abstract" && val.eq_ignore_ascii_case("true")
                    });

                    // Collect the entire def element's inner content
                    match parse_single_def(&mut reader, &tag_name, is_abstract, source) {
                        Some(parsed) => defs.push(parsed),
                        None if is_abstract => abstract_skipped += 1,
                        None => {}
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag_name == "Defs" {
                    inside_defs = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    (defs, abstract_skipped)
}

/// Parses a single def element that we've already seen the opening tag for.
/// Reads child elements to find `defName`, and reconstructs the raw XML string.
/// Returns `None` if the def is abstract and has no defName.
fn parse_single_def(
    reader: &mut Reader<&[u8]>,
    def_type: &str,
    is_abstract: bool,
    source: &str,
) -> Option<ParsedDef> {
    let mut def_name: Option<String> = None;
    let mut depth: u32 = 1;
    let mut inner_xml = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                // Build attributes string for raw XML reconstruction
                let attrs = build_attrs_string(e);
                inner_xml.push_str(&format!("<{tag}{attrs}>"));

                // If this is a direct child `defName`, read its text
                if depth == 2 && tag == "defName" {
                    let mut text_buf = Vec::new();
                    if let Ok(Event::Text(t)) = reader.read_event_into(&mut text_buf) {
                        let text = t.unescape().ok().map(|s| s.to_string());
                        if let Some(ref name) = text {
                            def_name = Some(name.clone());
                            inner_xml.push_str(name);
                        }
                    }
                    // Read the closing </defName>
                    let mut end_buf = Vec::new();
                    if let Ok(Event::End(_)) = reader.read_event_into(&mut end_buf) {
                        inner_xml.push_str(&format!("</{tag}>"));
                    }
                    depth -= 1;
                }
            }
            Ok(Event::End(ref e)) => {
                depth -= 1;
                if depth == 0 {
                    // We've closed the def element itself
                    break;
                }
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                inner_xml.push_str(&format!("</{tag}>"));
            }
            Ok(Event::Text(ref t)) => {
                if let Ok(text) = t.unescape() {
                    inner_xml.push_str(&text);
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs = build_attrs_string(e);
                inner_xml.push_str(&format!("<{tag}{attrs}/>"));
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    // Skip abstract defs that don't have a defName
    if is_abstract && def_name.is_none() {
        return None;
    }

    let def_name = def_name?;

    // Reconstruct the full raw XML for this def
    let raw_xml = format!("<{def_type}>{inner_xml}</{def_type}>");

    Some(ParsedDef {
        def_type: def_type.to_string(),
        def_name,
        raw_xml,
        source_file: source.to_string(),
    })
}

/// Builds an attribute string from a `BytesStart` element (e.g., ` key="value"`).
pub(crate) fn build_attrs_string(e: &quick_xml::events::BytesStart<'_>) -> String {
    let mut attrs = String::new();
    for attr in e.attributes().filter_map(|a| a.ok()) {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let val = String::from_utf8_lossy(&attr.value).to_string();
        attrs.push_str(&format!(r#" {key}="{val}""#));
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_defs_basic() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Defs>
    <ThingDef>
        <defName>Steel</defName>
        <label>steel</label>
        <statBases>
            <MaxHitPoints>100</MaxHitPoints>
        </statBases>
    </ThingDef>
    <ThingDef>
        <defName>Wood</defName>
        <label>wood</label>
        <statBases>
            <MaxHitPoints>65</MaxHitPoints>
        </statBases>
    </ThingDef>
</Defs>"#;

        let (defs, _) = parse_defs_from_xml(xml, "test.xml");
        assert_eq!(defs.len(), 2);

        assert_eq!(defs[0].def_type, "ThingDef");
        assert_eq!(defs[0].def_name, "Steel");
        assert_eq!(defs[0].source_file, "test.xml");
        assert!(defs[0].raw_xml.contains("<defName>Steel</defName>"));
        assert!(defs[0].raw_xml.contains("<MaxHitPoints>100</MaxHitPoints>"));

        assert_eq!(defs[1].def_type, "ThingDef");
        assert_eq!(defs[1].def_name, "Wood");
        assert!(defs[1].raw_xml.contains("<defName>Wood</defName>"));
    }

    #[test]
    fn skip_abstract_defs_without_def_name() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Defs>
    <ThingDef Abstract="True">
        <label>base thing</label>
        <statBases>
            <MaxHitPoints>50</MaxHitPoints>
        </statBases>
    </ThingDef>
    <ThingDef>
        <defName>Steel</defName>
        <label>steel</label>
    </ThingDef>
    <ThingDef Abstract="True">
        <defName>BaseResource</defName>
        <label>resource base</label>
    </ThingDef>
</Defs>"#;

        let (defs, _) = parse_defs_from_xml(xml, "test.xml");
        // Abstract without defName is skipped; abstract WITH defName is kept;
        // non-abstract with defName is kept.
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].def_name, "Steel");
        assert_eq!(defs[1].def_name, "BaseResource");
    }

    #[test]
    fn multiple_def_types() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Defs>
    <ThingDef>
        <defName>Steel</defName>
        <label>steel</label>
    </ThingDef>
    <RecipeDef>
        <defName>SmeltSteel</defName>
        <label>smelt steel</label>
    </RecipeDef>
</Defs>"#;

        let (defs, _) = parse_defs_from_xml(xml, "test.xml");
        assert_eq!(defs.len(), 2);

        assert_eq!(defs[0].def_type, "ThingDef");
        assert_eq!(defs[0].def_name, "Steel");

        assert_eq!(defs[1].def_type, "RecipeDef");
        assert_eq!(defs[1].def_name, "SmeltSteel");
    }

    #[test]
    fn parse_defs_dir_with_fixtures() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/mods/mod_core/Defs");

        if fixtures.exists() {
            let (defs, _, _errors) = parse_defs_dir(&fixtures);
            assert!(defs.len() >= 2, "Expected at least Steel and Wood defs");

            let steel = defs.iter().find(|d| d.def_name == "Steel");
            assert!(steel.is_some(), "Expected to find Steel def");

            let wood = defs.iter().find(|d| d.def_name == "Wood");
            assert!(wood.is_some(), "Expected to find Wood def");
        }
    }

    #[test]
    fn non_def_elements_skipped() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Defs>
    <SomeRandomElement>
        <defName>NotADef</defName>
    </SomeRandomElement>
    <ThingDef>
        <defName>Steel</defName>
    </ThingDef>
</Defs>"#;

        let (defs, _) = parse_defs_from_xml(xml, "test.xml");
        // "SomeRandomElement" doesn't end with "Def", so it should be skipped
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].def_name, "Steel");
    }

    #[test]
    fn parse_defs_returns_abstract_skip_count() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Defs>
    <ThingDef Abstract="True">
        <label>base thing</label>
    </ThingDef>
    <ThingDef Abstract="True">
        <label>another base</label>
    </ThingDef>
    <ThingDef>
        <defName>Steel</defName>
    </ThingDef>
    <ThingDef Abstract="True">
        <defName>HasName</defName>
        <label>named abstract</label>
    </ThingDef>
</Defs>"#;

        let (defs, skipped) = parse_defs_from_xml(xml, "test.xml");
        assert_eq!(defs.len(), 2, "Steel + HasName");
        assert_eq!(skipped, 2, "Two abstract defs without defName were skipped");
    }

    #[test]
    fn parse_defs_dir_returns_errors_vec() {
        let dir = tempfile::tempdir().unwrap();
        let defs_dir = dir.path().join("Defs");
        std::fs::create_dir_all(&defs_dir).unwrap();
        std::fs::write(
            defs_dir.join("Test.xml"),
            r#"<Defs><ThingDef><defName>Foo</defName></ThingDef></Defs>"#,
        )
        .unwrap();

        let (defs, skipped, errors) = parse_defs_dir(defs_dir.as_path());
        assert_eq!(defs.len(), 1);
        assert_eq!(skipped, 0);
        assert!(errors.is_empty());
    }
}
