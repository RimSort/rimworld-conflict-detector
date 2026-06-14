//! Priority and ordering extraction for Harmony patch attributes.
//!
//! Harmony supports three attributes that control patch execution order:
//! - `[HarmonyPriority(int)]` — sets the numeric priority (higher = runs first, default 400)
//! - `[HarmonyBefore(params string[])]` — list of Harmony IDs this patch should run before
//! - `[HarmonyAfter(params string[])]` — list of Harmony IDs this patch should run after
//!
//! These are separate attributes from `[HarmonyPatch]` and may appear on the same type.

use dotnetdll::prelude::*;

use crate::attributes::attribute_type_name;

/// The default Harmony priority (Normal).
const DEFAULT_PRIORITY: i32 = 400;

/// Attribute name substrings for priority/ordering attributes.
const HARMONY_PRIORITY: &str = "HarmonyPriority";
const HARMONY_BEFORE: &str = "HarmonyBefore";
const HARMONY_AFTER: &str = "HarmonyAfter";

/// Extracted priority and ordering information from Harmony attributes.
#[derive(Debug, Clone)]
pub struct PriorityInfo {
    /// Harmony execution priority. Higher values run first. Default is 400 (Normal).
    /// Known constants: Last=0, VeryLow=100, Low=200, Normal=400, High=600, First=800.
    pub priority: i32,
    /// Harmony IDs that this patch should run before.
    pub before: Vec<String>,
    /// Harmony IDs that this patch should run after.
    pub after: Vec<String>,
}

impl Default for PriorityInfo {
    fn default() -> Self {
        Self {
            priority: DEFAULT_PRIORITY,
            before: Vec::new(),
            after: Vec::new(),
        }
    }
}

/// Scan a type definition's attributes for `[HarmonyPriority]`, `[HarmonyBefore]`,
/// and `[HarmonyAfter]` and return the extracted info.
///
/// If none of these attributes are present, returns defaults (priority=400, empty vecs).
pub fn extract_priority_info(typedef: &TypeDefinition<'_>, res: &Resolution<'_>) -> PriorityInfo {
    let mut info = PriorityInfo::default();

    for attr in &typedef.attributes {
        let type_name = attribute_type_name(attr, res);

        if type_name.contains(HARMONY_PRIORITY) {
            extract_priority_value(attr, res, &mut info);
        } else if type_name.contains(HARMONY_BEFORE) {
            extract_string_array(attr, res, &mut info.before);
        } else if type_name.contains(HARMONY_AFTER) {
            extract_string_array(attr, res, &mut info.after);
        }
    }

    info
}

/// Extract the integer priority value from a `[HarmonyPriority(int)]` attribute.
///
/// The constructor takes a single `int` parameter. The C# compiler inlines
/// `Priority.*` constants (they're `const int`, not enum values), so we get
/// a plain `FixedArg::Integral(IntegralParam::Int32(value))`.
fn extract_priority_value(attr: &Attribute<'_>, res: &Resolution<'_>, info: &mut PriorityInfo) {
    let data = match attr.instantiation_data(&AlwaysFailsResolver, res) {
        Ok(data) => data,
        Err(_) => return,
    };

    if let Some(FixedArg::Integral(IntegralParam::Int32(value))) = data.constructor_args.first() {
        info.priority = *value;
    }
}

/// Extract a string array from a `[HarmonyBefore(params string[])]` or
/// `[HarmonyAfter(params string[])]` attribute.
///
/// The `params string[]` constructor parameter is encoded in metadata as a
/// single `FixedArg::Array(String, Some(vec![FixedArg::String(...)]))`.
fn extract_string_array(attr: &Attribute<'_>, res: &Resolution<'_>, target: &mut Vec<String>) {
    let data = match attr.instantiation_data(&AlwaysFailsResolver, res) {
        Ok(data) => data,
        Err(_) => return,
    };

    if let Some(FixedArg::Array(_, Some(elements))) = data.constructor_args.first() {
        for elem in elements {
            if let FixedArg::String(Some(s)) = elem {
                target.push(s.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dll_path() -> std::path::PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::path::PathBuf::from(manifest_dir)
            .join("../../tests/fixtures/harmony/dlls/TestPatches.dll")
    }

    #[test]
    fn extract_priority_from_priority_patch() {
        let bytes = std::fs::read(test_dll_path()).unwrap();
        let res = Resolution::parse(&bytes, ReadOptions::default()).unwrap();

        let typedef = res
            .enumerate_type_definitions()
            .find(|(_, td)| td.name == "PriorityPatch")
            .expect("PriorityPatch type not found")
            .1;

        let info = extract_priority_info(typedef, &res);

        // Priority.High = 600 in Harmony 2.x (higher = runs first)
        assert_eq!(info.priority, 600, "Priority.High should be 600");
        assert_eq!(
            info.before,
            vec!["other.mod"],
            "before should contain 'other.mod'"
        );
        assert_eq!(
            info.after,
            vec!["dependency.mod"],
            "after should contain 'dependency.mod'"
        );
    }

    #[test]
    fn extract_defaults_for_type_without_priority_attrs() {
        let bytes = std::fs::read(test_dll_path()).unwrap();
        let res = Resolution::parse(&bytes, ReadOptions::default()).unwrap();

        let typedef = res
            .enumerate_type_definitions()
            .find(|(_, td)| td.name == "SimplePatch")
            .expect("SimplePatch type not found")
            .1;

        let info = extract_priority_info(typedef, &res);

        assert_eq!(
            info.priority, 400,
            "Default priority should be 400 (Normal)"
        );
        assert!(info.before.is_empty(), "before should be empty by default");
        assert!(info.after.is_empty(), "after should be empty by default");
    }
}
