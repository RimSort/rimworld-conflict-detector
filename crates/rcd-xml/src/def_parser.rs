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
                let (parsed, skipped, xml_errors) = parse_defs_from_xml(&contents, &source);
                defs.extend(parsed);
                total_skipped += skipped;
                errors.extend(xml_errors);
            }
            Err(e) => {
                errors.push(format!("Failed to read {}: {}", path.display(), e));
            }
        }
    }

    (defs, total_skipped, errors)
}

/// One-event-per-line scan that guarantees a diagnostic for any malformed
/// document, even when structural parsing swallows the error in a best-effort
/// nested read (quick-xml can latch after Err, hiding it from outer loops).
///
/// Depth-aware (FR-7a I-3): quick-xml is non-validating, so a document that
/// merely ends inside open elements yields Ok(Eof) with no Err at all. The
/// scan tracks element depth and reports unclosed elements at Eof; the Err
/// message is byte-identical to the structural captures so the callers'
/// sort+dedup collapses the guaranteed duplicate pair (M-2).
pub(crate) fn check_well_formed(xml: &str, source: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth: u32 = 0;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => {
                return if depth > 0 {
                    Some(format!(
                        "XML parse error in {}: document ends with {} unclosed element(s) (results may be incomplete)",
                        source, depth
                    ))
                } else {
                    None
                };
            }
            Ok(_) => {}
            Err(e) => {
                return Some(format!("XML parse error in {}: {}", source, e));
            }
        }
        buf.clear();
    }
}

/// Parses XML content for defs. Finds top-level elements under `<Defs>` whose tag names
/// end with "Def" (e.g., `ThingDef`, `RecipeDef`). Extracts the `defName` child and
/// captures the raw XML of the entire def element.
///
/// Abstract defs (those with `Abstract="True"`) that lack a `defName` are skipped.
/// Defs whose defName identity is untrustworthy are also skipped — each such
/// drop is diagnosed in the returned errors (a poisoned ABSTRACT def also
/// surfaces in `abstract_skipped`, whose count conflates the two causes).
///
/// Returns a tuple of (parsed defs, count of abstract defs without defName that were skipped, parse errors).
pub fn parse_defs_from_xml(xml: &str, source: &str) -> (Vec<ParsedDef>, usize, Vec<String>) {
    let mut defs = Vec::new();
    let mut abstract_skipped: usize = 0;
    let mut errors: Vec<String> = Vec::new();
    if let Some(msg) = check_well_formed(xml, source) {
        errors.push(msg);
    }
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
                    match parse_single_def(&mut reader, &tag_name, is_abstract, source, &mut errors)
                    {
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
            Err(e) => {
                errors.push(format!("XML parse error in {}: {}", source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    // FR-7a M-2: the pre-check and a structural capture can report the same
    // root error at the same offset (same reader, same config); the messages
    // are byte-identical, so dedup collapses the guaranteed pair. The
    // depth-at-Eof diagnostic stays distinct — no structural site can
    // produce it (it is the absence of an Err).
    errors.sort();
    errors.dedup();

    (defs, abstract_skipped, errors)
}

/// Parses a single def element that we've already seen the opening tag for.
/// Reads child elements to find `defName`, and reconstructs the raw XML string.
/// Returns `None` if the def is abstract and has no defName, or if its
/// defName identity was poisoned (nested markup, undecodable text, or an
/// unclosed/mismatched close — each diagnosed in `errors`).
fn parse_single_def(
    reader: &mut Reader<&[u8]>,
    def_type: &str,
    is_abstract: bool,
    source: &str,
    errors: &mut Vec<String>,
) -> Option<ParsedDef> {
    let mut def_name: Option<String> = None;
    // Set when the defName identity is untrustworthy (see defName block below).
    // Function-scoped on purpose: any poison kills the identity from EVERY
    // sibling defName block; a block-local flag would let a clean second
    // block rescue a poisoned first one (see unclosed_defname_at_eof test).
    let mut poisoned = false;
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

                // If this is a direct child `defName`, accumulate until its
                // End: raw (escaped) bytes go to inner_xml, decoded text to the
                // identity. Consuming the End here — instead of the old
                // one-shot reads — is what keeps CDATA defNames resolving and
                // empty <defName></defName> from swallowing the next sibling.
                //
                // defName is a LEAF in RimWorld's schema: nested markup, a
                // close with the wrong name, EOF, or a decode failure make the
                // identity untrustworthy. def_name is the join key for
                // cross-mod conflict detection, so a truncated or fabricated
                // identity is worse than none — such defs are dropped, visibly.
                if depth == 2 && tag == "defName" {
                    let mut raw = String::new();
                    let mut decoded = String::new();
                    loop {
                        let mut inner_buf = Vec::new();
                        match reader.read_event_into(&mut inner_buf) {
                            Ok(Event::Text(ref t)) => {
                                raw.push_str(&String::from_utf8_lossy(t.as_ref()));
                                // trim() decouples identity from the reader's
                                // trim_text config (matches patch_ops convention).
                                match t.unescape() {
                                    Ok(name) => decoded.push_str(&name),
                                    Err(e) => {
                                        poisoned = true;
                                        errors.push(format!(
                                            "defName in {} could not be decoded: {}",
                                            source, e
                                        ));
                                    }
                                }
                            }
                            Ok(Event::CData(ref c)) => {
                                let content = String::from_utf8_lossy(c.as_ref());
                                raw.push_str(&format!("<![CDATA[{}]]>", content));
                                decoded.push_str(&content);
                            }
                            Ok(Event::End(ref end)) => {
                                if end.name().as_ref() == &b"defName"[..] {
                                    break;
                                }
                                // A non-matching close means we are past
                                // defName's end; anything concatenated so far
                                // is a fabricated identity. FR-7a M-1: when a
                                // nested Start already poisoned and diagnosed
                                // the root cause, this End is expected drain
                                // behavior — re-diagnosing here would
                                // misattribute a second cause.
                                if !poisoned {
                                    errors.push(format!(
                                        "defName in {} closed by unexpected </{}>; def dropped",
                                        source,
                                        String::from_utf8_lossy(end.name().as_ref())
                                    ));
                                }
                                poisoned = true;
                                break;
                            }
                            Ok(Event::Eof) | Err(_) => {
                                poisoned = true;
                                errors.push(format!(
                                    "defName in {} is not closed; def dropped",
                                    source
                                ));
                                break;
                            }
                            Ok(Event::Start(_)) | Ok(Event::Empty(_)) => {
                                // defName is a leaf: nested markup makes the
                                // identity untrustworthy. The matching End
                                // below exits WITHOUT a diagnostic, so
                                // diagnose at the first violation; then keep
                                // draining to the close.
                                if !poisoned {
                                    errors.push(format!(
                                        "defName in {} contains nested markup; defName is a leaf: def dropped",
                                        source
                                    ));
                                }
                                poisoned = true;
                            }
                            Ok(Event::Comment(_))
                            | Ok(Event::PI(_))
                            | Ok(Event::Decl(_))
                            | Ok(Event::DocType(_)) => {
                                // Comments and PIs carry no element content:
                                // skip, keep the def, identity from remaining
                                // Text/CData. Dropping a working def over an
                                // inline comment would recreate the false
                                // negatives this branch exists to eliminate.
                            } // No catch-all: quick-xml 0.37.5 emits no other
                              // Ok variants, so the arms above are exhaustive
                              // (a `_` here would be dead code, and dead code
                              // is where future event kinds would hide).
                        }
                        inner_buf.clear();
                    }
                    // Single push on every exit path, so the reconstruction
                    // stays well-formed even for defs we then drop.
                    inner_xml.push_str(&raw);
                    inner_xml.push_str(&format!("</{tag}>"));
                    if !poisoned {
                        let trimmed = decoded.trim();
                        if !trimmed.is_empty() {
                            def_name = Some(trimmed.to_string());
                        }
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
                // Preserve source escaping (see defName note above).
                inner_xml.push_str(&String::from_utf8_lossy(t.as_ref()));
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs = build_attrs_string(e);
                inner_xml.push_str(&format!("<{tag}{attrs}/>"));
            }
            Ok(Event::CData(ref c)) => {
                // Re-emit verbatim; `]]>` is illegal inside CDATA, so the
                // wrapper round-trips safely. Dropping CDATA masked real
                // divergences in the replacement diff.
                inner_xml.push_str(&format!(
                    "<![CDATA[{}]]>",
                    String::from_utf8_lossy(c.as_ref())
                ));
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(format!("XML parse error in {}: {}", source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    // Poisoned identity: drop the def. The diagnostic was pushed at the
    // poison site; never register a def under an untrustworthy name.
    if poisoned {
        return None;
    }

    // Abstract defs without a defName are legitimate bases — the caller
    // counts them as skipped. A concrete def with no defName is a defect:
    // diagnose it, never drop it silently. (The poisoned path returned above
    // with its own diagnostic; reaching here means the element was absent.)
    if is_abstract && def_name.is_none() {
        return None;
    }
    if def_name.is_none() {
        errors.push(format!(
            "def {} in {} has no defName; def dropped",
            def_type, source
        ));
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
        // Raw (still-escaped) source bytes; a literal `"` in them can only
        // mean the source used single quotes, so escaping it here is always
        // correct for the double-quoted re-emission.
        let val = String::from_utf8_lossy(&attr.value).replace('"', "&quot;");
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

        let (defs, _, errors) = parse_defs_from_xml(xml, "test.xml");
        assert!(errors.is_empty());
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

        let (defs, _, _) = parse_defs_from_xml(xml, "test.xml");
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

        let (defs, _, _) = parse_defs_from_xml(xml, "test.xml");
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

        let (defs, _, _) = parse_defs_from_xml(xml, "test.xml");
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

        let (defs, skipped, _) = parse_defs_from_xml(xml, "test.xml");
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

    #[test]
    fn malformed_defs_xml_reports_error_and_keeps_partial_results() {
        // First def is valid; the second is truncated mid opening tag, which
        // quick-xml reports as a syntax error ("tag not closed"). Truncation
        // after text with well-formed tags would NOT error (non-validating Eof).
        let xml = r#"<Defs>
    <ThingDef><defName>Good</defName></ThingDef>
    <ThingDef"#;

        let (defs, _skipped, errors) = parse_defs_from_xml(xml, "test.xml");
        assert!(
            defs.iter().any(|d| d.def_name == "Good"),
            "valid def before the error should be kept"
        );
        assert!(!errors.is_empty(), "parse error must be reported");
        assert!(errors.iter().all(|e| e.contains("test.xml")));
    }

    #[test]
    fn unclosed_cdata_yields_diagnostic_via_precheck() {
        // quick-xml latches on this error inside nested best-effort reads, so the
        // structural parser alone reports nothing; the pre-check must catch it.
        let xml = r#"<Defs><ThingDef><defName><![CDATA[Steel</defName><label>x</label></ThingDef></Defs>"#;
        let (_defs, _skipped, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(
            !errors.is_empty(),
            "pre-check must surface the malformed document"
        );
        assert!(errors.iter().all(|e| e.contains("t.xml")));
    }

    #[test]
    fn escaped_entities_survive_raw_xml_reconstruction() {
        let xml =
            r#"<Defs><ThingDef><defName>Steel</defName><label>a &lt; b</label></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty());
        assert_eq!(defs[0].def_name, "Steel");
        assert!(
            defs[0].raw_xml.contains("&lt;"),
            "raw_xml must keep source escaping: {}",
            defs[0].raw_xml
        );
    }

    #[test]
    fn defname_identity_decoded_but_raw_xml_keeps_escaping() {
        let xml =
            r#"<Defs><ThingDef><defName>a &lt; b</defName><label>x</label></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(defs[0].def_name, "a < b", "identity must be decoded");
        assert!(
            defs[0].raw_xml.contains("<defName>a &lt; b</defName>"),
            "raw_xml must keep escaping: {}",
            defs[0].raw_xml
        );
    }

    #[test]
    fn undecodable_defname_reports_instead_of_vanishing() {
        // Regression pin (passed pre-fix too): a single Text segment's
        // unescape is all-or-nothing, so this shape never registered a
        // partial identity. The discriminating multi-segment case is
        // cdata_then_bad_entity_drops_def.
        let xml = r#"<Defs><ThingDef><defName>bad &foo; entity</defName></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(defs.is_empty());
        assert!(
            errors.iter().any(|e| e.contains("could not be decoded")),
            "{errors:?}"
        );
    }

    #[test]
    fn cdata_content_survives_raw_xml_reconstruction() {
        let xml = r#"<Defs><ThingDef><defName>Steel</defName><label><![CDATA[a < b]]></label></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            defs[0].raw_xml.contains("<![CDATA[a < b]]>"),
            "raw_xml must carry CDATA verbatim: {}",
            defs[0].raw_xml
        );
        assert_eq!(
            defs[0].raw_xml.matches("<![CDATA[").count(),
            1,
            "must re-wrap once, not double-wrap: {}",
            defs[0].raw_xml
        );
    }

    #[test]
    fn cdata_defname_resolves_and_survives_reconstruction() {
        let xml = r#"<Defs><ThingDef><defName><![CDATA[Steel]]></defName><label>x</label></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].def_name, "Steel");
        assert!(
            defs[0]
                .raw_xml
                .contains("<defName><![CDATA[Steel]]></defName>"),
            "{}",
            defs[0].raw_xml
        );
    }

    #[test]
    fn empty_defname_does_not_swallow_following_sibling() {
        // Discriminating P0 regression: the old one-shot End read consumed the
        // NEXT Start event, so the damage was IN-DEF — sibling elements of the
        // same def were truncated out of raw_xml. (The outer loop resyncs at
        // the next def's Start, so later defs always survived; the previous
        // version of this test asserted only that, and passed pre-fix.)
        // Here the def keeps identity from the FIRST defName; the empty second
        // one must consume its own End and leave <label> intact.
        let xml = r#"<Defs><ThingDef><defName>A</defName><defName></defName><label>x</label></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].def_name, "A");
        assert!(
            defs[0].raw_xml.contains("<label>x</label>"),
            "sibling after empty defName must survive in raw_xml: {}",
            defs[0].raw_xml
        );
        assert!(
            check_well_formed(&defs[0].raw_xml, "raw").is_none(),
            "reconstruction must stay well-formed: {}",
            defs[0].raw_xml
        );
    }

    #[test]
    fn unclosed_defname_at_eof_drops_def() {
        // FR-2d gate I-1: a second defName left open at EOF used to break out
        // with a stale identity from the first defName sibling, so a kept def's
        // raw_xml ended half-open (`<defName>`) with zero diagnostics.
        let xml = r#"<Defs><ThingDef><defName>A</defName><defName>B"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(
            defs.is_empty(),
            "def with unclosed defName must be dropped: {:?}",
            defs.iter()
                .map(|d| (&d.def_name, &d.raw_xml))
                .collect::<Vec<_>>()
        );
        assert!(
            errors.iter().any(|e| e.contains("is not closed")),
            "unclosed defName must be diagnosed: {errors:?}"
        );
    }

    #[test]
    fn cdata_then_bad_entity_drops_def() {
        // FR-2d gate I-2: a decode failure on a LATER text segment used to
        // register the def under the earlier segment's partial identity
        // ("ok") — a renamed ghost def is worse than a dropped one, since
        // defName is the join key for cross-mod conflict detection.
        let xml = r#"<Defs><ThingDef><defName><![CDATA[ok]]>&foo;</defName></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(
            defs.is_empty(),
            "partial identity must not survive: {:?}",
            defs.iter().map(|d| &d.def_name).collect::<Vec<_>>()
        );
        assert!(
            errors.iter().any(|e| e.contains("could not be decoded")),
            "decode failure must be diagnosed: {errors:?}"
        );
    }

    #[test]
    fn nested_element_in_defname_drops_def() {
        // FR-2d gate M-1: a non-matching End used to close the defName block
        // early, fabricating a concatenated identity ("Ax") and later leaking
        // the real </defName> into raw_xml as a duplicate close. defName is a
        // leaf in RimWorld's schema: any nested markup poisons identity.
        let xml = r#"<Defs><ThingDef><defName>A<b>x</b>&foo;</defName><label>y</label></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(
            defs.is_empty(),
            "non-leaf defName must drop the def, not fabricate an identity: {:?}",
            defs.iter()
                .map(|d| (&d.def_name, &d.raw_xml))
                .collect::<Vec<_>>()
        );
        // FR-2f: the drop itself was never the contract — the VISIBLE drop
        // was. Asserting only the def vanished is what let the silent
        // poison path ship green in FR-2e.
        assert!(
            errors.iter().any(|e| e.contains("nested markup")),
            "non-leaf defName must be diagnosed, not just dropped: {errors:?}"
        );
        // FR-7a M-1: the nested Start(b) already diagnosed the root cause; the
        // drain's non-matching End(b) must not misattribute a second cause.
        assert!(
            !errors.iter().any(|e| e.contains("closed by unexpected")),
            "exactly one drop diagnostic per cause: {errors:?}"
        );
    }

    #[test]
    fn truncation_after_closed_defname_is_diagnosed() {
        // FR-7a I-3: quick-xml is non-validating, so this document yields
        // Ok(Eof) with no Err anywhere — the flat pre-check saw nothing and
        // the def was KEPT with a fabricated, fully-closed raw_xml. Cross-mod
        // diffing then compared that fabrication against complete defs
        // (replacement.rs may still fire on it — the contract here is
        // VISIBILITY: the depth diagnostic attributes any resulting conflict
        // to the truncated source file).
        let xml = r#"<Defs><ThingDef><defName>Steel</defName>"#;
        let (_defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(
            errors.iter().any(|e| e.contains("unclosed element")),
            "pre-check must catch depth-at-Eof: {errors:?}"
        );
    }

    #[test]
    fn nested_markup_in_defname_drops_with_diagnostic() {
        // FR-2e gate I-1: the leaf-violation arm poisoned silently when the
        // block exited via the MATCHING </defName> (the drain comment's
        // "mismatched End or EOF will break" was false) — defs vanished with
        // zero diagnostics. Both markup shapes must now diagnose at the
        // first violation.
        for xml in [
            r#"<Defs><ThingDef><defName><v/></defName></ThingDef></Defs>"#,
            r#"<Defs><ThingDef><defName>A<b>x</b></defName></ThingDef></Defs>"#,
        ] {
            let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
            assert!(defs.is_empty(), "{xml}");
            assert!(
                errors.iter().any(|e| e.contains("nested markup")),
                "leaf-violation drop must be diagnosed: {xml} errors={errors:?}"
            );
        }
    }

    #[test]
    fn concrete_def_without_defname_is_diagnosed() {
        // FR-2f gate P2: a CONCRETE def with no defName at all was the last
        // silent return-None path in parse_single_def (abstract skips are
        // counted by the caller; concrete drops were invisible).
        let xml = r#"<Defs><ThingDef><label>x</label></ThingDef></Defs>"#;
        let (defs, _skipped, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(defs.is_empty());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("ThingDef") && e.contains("has no defName")),
            "concrete def without defName must be diagnosed, not vanish: errors={errors:?}"
        );
    }

    #[test]
    fn comment_inside_defname_is_skipped_not_fatal() {
        // RimWorld mods really write this. Comments carry no element content:
        // skip them, keep the def, identity from the remaining text.
        let xml = r#"<Defs><ThingDef><defName> <!-- legacy --> Steel</defName><label>x</label></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].def_name, "Steel");
        assert!(
            check_well_formed(&defs[0].raw_xml, "raw").is_none(),
            "{}",
            defs[0].raw_xml
        );
    }

    #[test]
    fn every_returned_def_raw_xml_is_well_formed() {
        // Reconstruction invariant behind all FR-2* fixes: replacement.rs
        // re-parses raw_xml, so any kept def's reconstruction must itself be
        // well-formed. Mix of plain, escaped, CDATA, and attributed content.
        let xml = r#"<Defs>
    <ThingDef Name="base"><defName>Steel</defName><label>a &lt; b</label></ThingDef>
    <ThingDef><defName>Wood</defName><costList><li Name="x">1</li></costList><desc><![CDATA[raw <stuff>]]></desc></ThingDef>
</Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(defs.len(), 2);
        for d in &defs {
            if let Some(msg) = check_well_formed(&d.raw_xml, "raw") {
                panic!("reconstruction not well-formed ({}): {msg}", d.def_name);
            }
        }
    }

    #[test]
    fn single_quoted_attr_with_embedded_quote_is_requoted_safely() {
        // Source Name='a "b"' → raw bytes contain a literal quote; re-emitting
        // into double quotes without escaping produces malformed XML.
        let xml = r#"<Defs><ThingDef><defName>Steel</defName><costList><li Name='a "b"'>x</li></costList></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            defs[0].raw_xml.contains(r#"Name="a &quot;b&quot;"#),
            "embedded quotes must be escaped on re-quote: {}",
            defs[0].raw_xml
        );
    }

    #[test]
    fn attribute_escaping_survives_raw_xml_reconstruction() {
        // NOTE: exercises build_attrs_string via CHILD elements. Root def opening
        // tags drop their attributes entirely (`<{{def_type}}>` reconstruction) —
        // that's the separate tracked P0 item, out of scope here.
        let xml = r#"<Defs><ThingDef><defName>Steel</defName><costList><li Name="a &lt; b">x</li></costList></ThingDef></Defs>"#;
        let (defs, _s, errors) = parse_defs_from_xml(xml, "t.xml");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            defs[0].raw_xml.contains(r#"Name="a &lt; b""#),
            "build_attrs_string must emit raw (escaped) attribute bytes: {}",
            defs[0].raw_xml
        );
    }
}
