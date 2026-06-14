use std::path::Path;

use dotnetdll::prelude::*;
use rcd_core::conflict::HarmonyPatchKind;

/// A Harmony patch discovered by parsing [HarmonyPatch] attributes from a .NET DLL.
#[derive(Debug, Clone)]
pub struct DiscoveredPatch {
    /// The fully qualified target type name (e.g. "System.String").
    pub target_type: Option<String>,
    /// The target method name (e.g. "Concat").
    pub target_method: Option<String>,
    /// Parameter types for overload disambiguation, if specified.
    pub parameter_types: Option<Vec<String>>,
    /// The kind of patch (Prefix, Postfix, Transpiler, Finalizer).
    pub patch_kind: HarmonyPatchKind,
    /// Harmony execution priority. Default is 400 (Normal).
    pub priority: i32,
    /// Harmony IDs that this patch should run before.
    pub before: Vec<String>,
    /// Harmony IDs that this patch should run after.
    pub after: Vec<String>,
    /// Whether the patch method returns bool (short-circuit risk for Prefix patches).
    pub returns_bool: bool,
    /// The DLL filename this patch was found in.
    pub dll_name: String,
}

/// Errors that can occur when parsing Harmony patches from a DLL.
#[derive(Debug)]
pub enum HarmonyParseError {
    Io(std::io::Error),
    DllParse(String),
}

impl std::fmt::Display for HarmonyParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarmonyParseError::Io(e) => write!(f, "I/O error: {}", e),
            HarmonyParseError::DllParse(e) => write!(f, "DLL parse error: {}", e),
        }
    }
}

impl std::error::Error for HarmonyParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HarmonyParseError::Io(e) => Some(e),
            HarmonyParseError::DllParse(_) => None,
        }
    }
}

impl From<std::io::Error> for HarmonyParseError {
    fn from(e: std::io::Error) -> Self {
        HarmonyParseError::Io(e)
    }
}

/// The default Harmony priority (Normal).
const DEFAULT_PRIORITY: i32 = 400;

/// Harmony attribute names we care about, matched by substring.
const HARMONY_PATCH: &str = "HarmonyPatch";
const HARMONY_PREFIX: &str = "HarmonyPrefix";
const HARMONY_POSTFIX: &str = "HarmonyPostfix";
const HARMONY_TRANSPILER: &str = "HarmonyTranspiler";
const HARMONY_FINALIZER: &str = "HarmonyFinalizer";

/// Parse all Harmony [HarmonyPatch] attributes from a .NET DLL.
///
/// Returns a `DiscoveredPatch` for each (target type+method, patch kind) combination found.
/// Types annotated with [HarmonyPatch] that have multiple patch methods (e.g. both
/// a Prefix and Postfix) produce one DiscoveredPatch per patch method.
pub fn parse_harmony_patches(dll_path: &Path) -> Result<Vec<DiscoveredPatch>, HarmonyParseError> {
    let bytes = std::fs::read(dll_path)?;
    let res = Resolution::parse(&bytes, ReadOptions::default())
        .map_err(|e| HarmonyParseError::DllParse(format!("{}", e)))?;

    let dll_name = dll_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.dll")
        .to_string();

    let mut patches = Vec::new();

    for (type_idx, typedef) in res.enumerate_type_definitions() {
        // Skip the <Module> pseudo-type (always at index 0 in .NET metadata)
        if typedef.name == "<Module>" {
            continue;
        }

        // Check if this type has any [HarmonyPatch] attributes
        let harmony_patch_attrs: Vec<_> = typedef
            .attributes
            .iter()
            .filter(|a| is_attr_named(a, &res, HARMONY_PATCH))
            .collect();

        if harmony_patch_attrs.is_empty() {
            continue;
        }

        // Merge target info from all [HarmonyPatch] attributes on this type.
        // Multiple attributes can be stacked to specify type, method, and params separately.
        let mut target_type: Option<String> = None;
        let mut target_method: Option<String> = None;
        let mut parameter_types: Option<Vec<String>> = None;

        for attr in &harmony_patch_attrs {
            merge_harmony_patch_args(
                attr,
                &res,
                &mut target_type,
                &mut target_method,
                &mut parameter_types,
            );
        }

        // Scan methods for patch-kind attributes ([HarmonyPrefix], [HarmonyPostfix], etc.)
        let mut found_patch_methods = Vec::new();

        for (_method_idx, method) in res.enumerate_methods(type_idx) {
            let kind = method_patch_kind(method, &res);
            if let Some(patch_kind) = kind {
                let returns_bool = method_returns_bool(method);
                found_patch_methods.push((patch_kind, returns_bool));
            }
        }

        // If no explicit patch method attributes found, this type might use convention-based
        // naming (a method named "Prefix", "Postfix", etc. without the attribute).
        // We still check for those as a fallback.
        if found_patch_methods.is_empty() {
            for (_method_idx, method) in res.enumerate_methods(type_idx) {
                let kind = match method.name.as_ref() {
                    "Prefix" => Some(HarmonyPatchKind::Prefix),
                    "Postfix" => Some(HarmonyPatchKind::Postfix),
                    "Transpiler" => Some(HarmonyPatchKind::Transpiler),
                    "Finalizer" => Some(HarmonyPatchKind::Finalizer),
                    _ => None,
                };
                if let Some(patch_kind) = kind {
                    let returns_bool = method_returns_bool(method);
                    found_patch_methods.push((patch_kind, returns_bool));
                }
            }
        }

        // Create a DiscoveredPatch for each patch method
        for (patch_kind, returns_bool) in &found_patch_methods {
            patches.push(DiscoveredPatch {
                target_type: target_type.clone(),
                target_method: target_method.clone(),
                parameter_types: parameter_types.clone(),
                patch_kind: *patch_kind,
                priority: DEFAULT_PRIORITY,
                before: Vec::new(),
                after: Vec::new(),
                returns_bool: *returns_bool,
                dll_name: dll_name.clone(),
            });
        }
    }

    Ok(patches)
}

/// Get the fully qualified type name of an attribute's constructor.
fn attribute_type_name<'a>(attr: &Attribute<'a>, res: &Resolution<'a>) -> String {
    match &attr.constructor {
        UserMethod::Definition(method_idx) => res[method_idx.parent_type()].type_name(),
        UserMethod::Reference(method_ref_idx) => {
            let method_ref = &res[*method_ref_idx];
            match &method_ref.parent {
                MethodReferenceParent::Type(method_type) => method_type.show(res),
                MethodReferenceParent::Module(mod_ref_idx) => res[*mod_ref_idx].name.to_string(),
                MethodReferenceParent::VarargMethod(method_idx) => {
                    res[method_idx.parent_type()].type_name()
                }
            }
        }
    }
}

/// Check if an attribute's constructor type name contains the given name.
fn is_attr_named(attr: &Attribute<'_>, res: &Resolution<'_>, name: &str) -> bool {
    let type_name = attribute_type_name(attr, res);
    type_name.contains(name)
}

/// Extract target type, method name, and parameter types from a [HarmonyPatch] attribute's
/// constructor arguments. Merges into existing values (for stacked attributes).
///
/// Handles decode failures gracefully — the attribute is still identified by its constructor,
/// but args are left as unknown (the existing values are not modified).
fn merge_harmony_patch_args(
    attr: &Attribute<'_>,
    res: &Resolution<'_>,
    target_type: &mut Option<String>,
    target_method: &mut Option<String>,
    parameter_types: &mut Option<Vec<String>>,
) {
    let data = match attr.instantiation_data(&AlwaysFailsResolver, res) {
        Ok(data) => data,
        Err(_) => {
            // Known limitation: dotnetdll's SerString parser can't handle long strings
            // (e.g. generic typeof() args). The attribute is still identifiable, we just
            // can't decode its constructor arguments.
            return;
        }
    };

    for arg in &data.constructor_args {
        match arg {
            FixedArg::Type(type_name) => {
                // typeof() argument — assembly-qualified type name.
                // Extract just the type name (before the first comma).
                let simple_name = type_name.split(',').next().unwrap_or(type_name);
                if target_type.is_none() {
                    *target_type = Some(simple_name.to_string());
                }
            }
            FixedArg::String(Some(s)) => {
                // String argument — this is the method name.
                if target_method.is_none() {
                    *target_method = Some(s.to_string());
                }
            }
            FixedArg::Array(_, Some(elements)) => {
                // Type[] argument — parameter types for overload disambiguation.
                if parameter_types.is_none() {
                    let types: Vec<String> = elements
                        .iter()
                        .filter_map(|elem| match elem {
                            FixedArg::Type(type_name) => {
                                let simple = type_name.split(',').next().unwrap_or(type_name);
                                Some(simple.to_string())
                            }
                            _ => None,
                        })
                        .collect();
                    if !types.is_empty() {
                        *parameter_types = Some(types);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Determine the patch kind from a method's attributes.
/// Returns None if the method has no Harmony patch-kind attribute.
fn method_patch_kind(method: &Method<'_>, res: &Resolution<'_>) -> Option<HarmonyPatchKind> {
    for attr in &method.attributes {
        let type_name = attribute_type_name(attr, res);
        if type_name.contains(HARMONY_PREFIX) {
            return Some(HarmonyPatchKind::Prefix);
        }
        if type_name.contains(HARMONY_POSTFIX) {
            return Some(HarmonyPatchKind::Postfix);
        }
        if type_name.contains(HARMONY_TRANSPILER) {
            return Some(HarmonyPatchKind::Transpiler);
        }
        if type_name.contains(HARMONY_FINALIZER) {
            return Some(HarmonyPatchKind::Finalizer);
        }
    }
    None
}

/// Check if a method's return type is `bool` (System.Boolean).
///
/// For Harmony Prefix patches, returning `false` skips the original method,
/// so a bool return type indicates a potential short-circuit risk.
fn method_returns_bool(method: &Method<'_>) -> bool {
    match &method.signature.return_type.1 {
        Some(ParameterType::Value(method_type)) => matches!(
            method_type,
            MethodType::Base(b) if matches!(b.as_ref(), BaseType::Boolean)
        ),
        _ => false,
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
    fn parse_simple_patch() {
        let patches = parse_harmony_patches(&test_dll_path()).unwrap();

        // SimplePatch targets System.String::Concat with both Prefix and Postfix
        let simple_prefix: Vec<_> = patches
            .iter()
            .filter(|p| {
                p.target_type.as_deref() == Some("System.String")
                    && p.target_method.as_deref() == Some("Concat")
                    && p.patch_kind == HarmonyPatchKind::Prefix
            })
            .collect();

        assert_eq!(
            simple_prefix.len(),
            1,
            "Should find exactly one Prefix for SimplePatch"
        );
        assert!(
            simple_prefix[0].returns_bool,
            "SimplePatch Prefix should return bool (short-circuit risk)"
        );
        assert_eq!(simple_prefix[0].priority, DEFAULT_PRIORITY);

        let simple_postfix: Vec<_> = patches
            .iter()
            .filter(|p| {
                p.target_type.as_deref() == Some("System.String")
                    && p.target_method.as_deref() == Some("Concat")
                    && p.patch_kind == HarmonyPatchKind::Postfix
            })
            .collect();

        assert_eq!(
            simple_postfix.len(),
            1,
            "Should find exactly one Postfix for SimplePatch"
        );
        assert!(
            !simple_postfix[0].returns_bool,
            "SimplePatch Postfix should not return bool"
        );
    }

    #[test]
    fn parse_overload_patch() {
        let patches = parse_harmony_patches(&test_dll_path()).unwrap();

        // OverloadPatch targets System.String::Format with parameter types [System.String, System.Object]
        let overload: Vec<_> = patches
            .iter()
            .filter(|p| {
                p.target_type.as_deref() == Some("System.String")
                    && p.target_method.as_deref() == Some("Format")
            })
            .collect();

        assert!(
            !overload.is_empty(),
            "Should find at least one patch for OverloadPatch"
        );

        let patch = &overload[0];
        assert!(
            patch.parameter_types.is_some(),
            "OverloadPatch should have parameter_types"
        );
        let param_types = patch.parameter_types.as_ref().unwrap();
        assert_eq!(param_types.len(), 2, "Should have 2 parameter types");
        assert_eq!(param_types[0], "System.String");
        assert_eq!(param_types[1], "System.Object");
    }

    #[test]
    fn parse_annotation_only_patch() {
        let patches = parse_harmony_patches(&test_dll_path()).unwrap();

        // AnnotationOnlyPatch has [HarmonyPatch] with no args — should still be found
        // but with no target info
        let anno: Vec<_> = patches
            .iter()
            .filter(|p| p.target_type.is_none() && p.target_method.is_none())
            .collect();

        // Should find patches from AnnotationOnlyPatch (it has a Prefix method by convention)
        assert!(
            !anno.is_empty(),
            "Should find patches from AnnotationOnlyPatch (no target info)"
        );
    }

    #[test]
    fn parse_split_attribute_patch() {
        let patches = parse_harmony_patches(&test_dll_path()).unwrap();

        // SplitAttributePatch has stacked [HarmonyPatch] attributes:
        //   [HarmonyPatch(typeof(List<string>))]  -- this one fails to decode (dotnetdll bug)
        //   [HarmonyPatch("Add")]                 -- this one succeeds
        // We should still pick up the method name from the second attribute
        let split: Vec<_> = patches
            .iter()
            .filter(|p| p.target_method.as_deref() == Some("Add"))
            .collect();

        assert!(
            !split.is_empty(),
            "Should find SplitAttributePatch with method name 'Add'"
        );
        // The target_type might be None due to the dotnetdll SerString bug with generic types
    }

    #[test]
    fn nonexistent_dll_returns_io_error() {
        let result = parse_harmony_patches(Path::new("/nonexistent/path/test.dll"));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HarmonyParseError::Io(_)));
    }

    #[test]
    fn dll_name_is_populated() {
        let patches = parse_harmony_patches(&test_dll_path()).unwrap();
        for patch in &patches {
            assert_eq!(patch.dll_name, "TestPatches.dll");
        }
    }
}
