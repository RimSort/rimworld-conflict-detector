// XPath normalization and overlap detection for RimWorld mod patches

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedXPath {
    pub def_type: Option<String>,
    pub def_name: Option<String>,
    pub field_path: Option<String>,
    pub raw: String,
}

/// Normalize a RimWorld XPath expression into its components.
///
/// Example: `Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints`
/// → def_type: "ThingDef", def_name: "Steel", field_path: "statBases/MaxHitPoints"
///
/// Handles patterns:
/// - `Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints` — full path with predicate
/// - `Defs/ThingDef[defName="Steel"]` — just the def, no field path
/// - `Defs/ThingDef/statBases` — def type without defName predicate
pub fn normalize_xpath(xpath: &str) -> NormalizedXPath {
    let raw = xpath.to_string();
    let segments: Vec<&str> = xpath.split('/').collect();

    let mut def_type = None;
    let mut def_name = None;
    let mut field_path_start = None;

    for (i, segment) in segments.iter().enumerate() {
        // Skip "Defs" segment
        if *segment == "Defs" {
            continue;
        }

        // Look for a segment ending with "Def" (might have predicate)
        if segment.contains("Def") {
            if let Some((dt, dn)) = parse_def_predicate(segment) {
                def_type = Some(dt);
                def_name = dn;
                field_path_start = Some(i + 1);
                break;
            }
        }
    }

    // Collect remaining segments as field path
    let field_path = if let Some(start) = field_path_start {
        if start < segments.len() {
            Some(segments[start..].join("/"))
        } else {
            None
        }
    } else {
        None
    };

    NormalizedXPath {
        def_type,
        def_name,
        field_path,
        raw,
    }
}

/// Extract def type and optional def name from a segment like `ThingDef[defName="Steel"]`.
///
/// Returns (def_type, optional def_name).
fn parse_def_predicate(part: &str) -> Option<(String, Option<String>)> {
    if let Some(bracket_pos) = part.find('[') {
        // Has a predicate
        let def_type = part[..bracket_pos].to_string();
        let predicate = &part[bracket_pos..];
        let def_name = extract_def_name_predicate(predicate);
        Some((def_type, def_name))
    } else if part.ends_with("Def") {
        // No predicate, just the def type
        Some((part.to_string(), None))
    } else {
        None
    }
}

/// Extract the defName value from a predicate like `[defName="Steel"]` or `[defName='Steel']`.
fn extract_def_name_predicate(predicate: &str) -> Option<String> {
    // Look for defName="value" or defName='value' or defName = "value"
    let predicate = predicate.trim();

    // Find defName
    if let Some(defname_pos) = predicate.find("defName") {
        let after_defname = &predicate[defname_pos + "defName".len()..];

        // Skip whitespace and =
        let after_defname = after_defname.trim_start();
        if !after_defname.starts_with('=') {
            return None;
        }
        let after_eq = after_defname[1..].trim_start();

        // Extract quoted value
        if after_eq.starts_with('"') {
            // Find closing quote
            if let Some(end_quote) = after_eq[1..].find('"') {
                return Some(after_eq[1..end_quote + 1].to_string());
            }
        } else if after_eq.starts_with('\'') {
            // Find closing quote
            if let Some(end_quote) = after_eq[1..].find('\'') {
                return Some(after_eq[1..end_quote + 1].to_string());
            }
        }
    }

    None
}

/// Check if two normalized XPaths target overlapping nodes.
///
/// Overlap means:
/// 1. Same def_type and def_name (both must be Some and equal)
/// 2. Field paths overlap: either one is None (targets whole def), or one field path
///    is a prefix of the other BY PATH SEGMENTS (not string prefix).
///
/// Uses segment-by-segment comparison to avoid false positives like "statBases" vs "stat".
pub fn xpaths_overlap(a: &NormalizedXPath, b: &NormalizedXPath) -> bool {
    // Must have matching def_type and def_name
    match (&a.def_type, &b.def_type) {
        (Some(dt_a), Some(dt_b)) if dt_a == dt_b => {}
        _ => return false,
    }

    match (&a.def_name, &b.def_name) {
        (Some(dn_a), Some(dn_b)) if dn_a == dn_b => {}
        (None, _) | (_, None) => {} // None = wildcard, targets all defs of this type
        _ => return false,
    }

    // Check field path overlap
    match (&a.field_path, &b.field_path) {
        // If either has no field path, they target the whole def — overlap
        (None, _) | (_, None) => true,

        // Both have field paths — check segment-by-segment prefix
        (Some(fa), Some(fb)) => {
            let segs_a: Vec<&str> = fa.split('/').collect();
            let segs_b: Vec<&str> = fb.split('/').collect();
            let min_len = segs_a.len().min(segs_b.len());

            segs_a[..min_len] == segs_b[..min_len]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_full_xpath() {
        let n = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases/MaxHitPoints");
        assert_eq!(n.def_type, Some("ThingDef".into()));
        assert_eq!(n.def_name, Some("Steel".into()));
        assert_eq!(n.field_path, Some("statBases/MaxHitPoints".into()));
    }

    #[test]
    fn normalize_xpath_no_field() {
        let n = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]");
        assert_eq!(n.def_type, Some("ThingDef".into()));
        assert_eq!(n.def_name, Some("Steel".into()));
        assert_eq!(n.field_path, None);
    }

    #[test]
    fn normalize_xpath_no_predicate() {
        let n = normalize_xpath("Defs/ThingDef/statBases");
        assert_eq!(n.def_type, Some("ThingDef".into()));
        assert_eq!(n.def_name, None);
        assert_eq!(n.field_path, Some("statBases".into()));
    }

    #[test]
    fn overlapping_same_path() {
        let a = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases/MaxHitPoints");
        let b = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases/MaxHitPoints");
        assert!(xpaths_overlap(&a, &b));
    }

    #[test]
    fn overlapping_prefix() {
        let a = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases");
        let b = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases/MaxHitPoints");
        assert!(xpaths_overlap(&a, &b));
    }

    #[test]
    fn non_overlapping_different_fields() {
        let a = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases/MaxHitPoints");
        let b = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/costList");
        assert!(!xpaths_overlap(&a, &b));
    }

    #[test]
    fn non_overlapping_different_defs() {
        let a = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases");
        let b = normalize_xpath("Defs/ThingDef[defName=\"Wood\"]/statBases");
        assert!(!xpaths_overlap(&a, &b));
    }

    #[test]
    fn wildcard_xpath_overlaps_with_specific() {
        let a = normalize_xpath("Defs/ThingDef/statBases");
        let b = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases");
        assert!(
            xpaths_overlap(&a, &b),
            "Wildcard (no defName) should overlap with specific def"
        );
    }

    #[test]
    fn wildcard_xpath_overlaps_with_wildcard() {
        let a = normalize_xpath("Defs/ThingDef/statBases");
        let b = normalize_xpath("Defs/ThingDef/costList");
        assert!(
            !xpaths_overlap(&a, &b),
            "Same wildcard type but different fields should not overlap"
        );
    }

    #[test]
    fn non_overlapping_string_prefix_but_different_segment() {
        // "statBases" starts_with "stat" as a string, but they are different path segments
        let a = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/statBases");
        let b = normalize_xpath("Defs/ThingDef[defName=\"Steel\"]/stat");
        assert!(!xpaths_overlap(&a, &b));
    }
}
