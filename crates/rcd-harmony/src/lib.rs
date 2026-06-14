// RimWorld Conflict Detector - Harmony analysis

pub mod attributes;
pub mod conflict;
pub mod discovery;
pub mod manual;
pub mod priority;

use std::panic;

use rcd_core::{
    Conflict, Detector, Diagnostic, DiagnosticSource, LoadOrder, Mod, ModSummary, Severity,
};

use crate::attributes::DiscoveredPatch;
use crate::conflict::detect_harmony_conflicts;
use crate::discovery::discover_assemblies;

/// Detector that finds Harmony patch conflicts across mods.
///
/// For each mod, discovers assemblies, parses Harmony patch attributes from DLLs,
/// then detects conflicts where multiple mods patch the same method.
pub struct HarmonyDetector;

impl Detector for HarmonyDetector {
    fn name(&self) -> &str {
        "harmony"
    }

    fn analyze(&self, mods: &[Mod], order: Option<&LoadOrder>) -> (Vec<Conflict>, Vec<Diagnostic>) {
        let mut all_patches: Vec<(ModSummary, Vec<DiscoveredPatch>)> = Vec::new();
        let mut diagnostics: Vec<Diagnostic> = Vec::new();

        for m in mods {
            let mod_summary = ModSummary::from(m);
            let mut mod_patches: Vec<DiscoveredPatch> = Vec::new();

            for assemblies_path in &m.assemblies_paths {
                let dlls = discover_assemblies(assemblies_path);

                for dll_path in dlls {
                    let dll_name = dll_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown.dll")
                        .to_string();

                    // Wrap parse_harmony_patches in catch_unwind since dotnetdll is
                    // pre-1.0 and may panic on malformed DLLs.
                    let parse_result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        crate::attributes::parse_harmony_patches(&dll_path)
                    }));

                    match parse_result {
                        Ok(Ok(patches)) => {
                            mod_patches.extend(patches);
                        }
                        Ok(Err(e)) => {
                            diagnostics.push(Diagnostic {
                                affected_mod: Some(mod_summary.package_id.clone()),
                                severity: Severity::Warning,
                                message: format!(
                                    "Failed to parse Harmony patches from {}: {}",
                                    dll_name, e
                                ),
                                source: DiagnosticSource::DllParsing,
                            });
                        }
                        Err(_) => {
                            diagnostics.push(Diagnostic {
                                affected_mod: Some(mod_summary.package_id.clone()),
                                severity: Severity::Warning,
                                message: format!(
                                    "DLL parser panicked while parsing {} (dotnetdll crash caught)",
                                    dll_name
                                ),
                                source: DiagnosticSource::DllParsing,
                            });
                        }
                    }
                }
            }

            if !mod_patches.is_empty() {
                all_patches.push((mod_summary, mod_patches));
            }
        }

        let conflicts = detect_harmony_conflicts(&all_patches, order);
        (conflicts, diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use dotnetdll::prelude::*;

    // -------------------------------------------------------------------------
    // SPIKE: Validate that dotnetdll 0.2.0 can parse Harmony [HarmonyPatch]
    // attributes from compiled .NET DLLs and extract their constructor arguments.
    //
    // This test was written as an exploratory spike to validate the viability of
    // using dotnetdll for Harmony patch detection.
    // -------------------------------------------------------------------------

    /// Helper: get the attribute constructor's type name from a Resolution.
    /// The constructor is a UserMethod that points to either a MethodDefinition
    /// or MethodReference. We need the parent type name to identify which
    /// attribute class it belongs to (e.g., "HarmonyPatchAttribute").
    fn attribute_type_name<'a>(attr: &Attribute<'a>, res: &Resolution<'a>) -> String {
        match &attr.constructor {
            UserMethod::Definition(method_idx) => {
                // Constructor is defined in this assembly — get the parent type name
                res[method_idx.parent_type()].type_name()
            }
            UserMethod::Reference(method_ref_idx) => {
                // Constructor is defined in an external assembly (the Harmony library)
                // The parent is a MethodReferenceParent which is typically Type(MethodType)
                let method_ref = &res[*method_ref_idx];
                match &method_ref.parent {
                    MethodReferenceParent::Type(method_type) => method_type.show(res),
                    MethodReferenceParent::Module(mod_ref_idx) => {
                        res[*mod_ref_idx].name.to_string()
                    }
                    MethodReferenceParent::VarargMethod(method_idx) => {
                        res[method_idx.parent_type()].type_name()
                    }
                }
            }
        }
    }

    /// Helper: check if an attribute is a HarmonyPatch attribute.
    fn is_harmony_patch_attr(attr: &Attribute<'_>, res: &Resolution<'_>) -> bool {
        let name = attribute_type_name(attr, res);
        // The type name in metadata may be fully qualified or short.
        // Harmony uses "HarmonyLib.HarmonyPatch" as namespace.name.
        name.contains("HarmonyPatch")
    }

    /// Helper: check if an attribute is a HarmonyPrefix attribute.
    fn is_harmony_prefix_attr(attr: &Attribute<'_>, res: &Resolution<'_>) -> bool {
        let name = attribute_type_name(attr, res);
        name.contains("HarmonyPrefix")
    }

    /// Helper: check if an attribute is a HarmonyPostfix attribute.
    fn is_harmony_postfix_attr(attr: &Attribute<'_>, res: &Resolution<'_>) -> bool {
        let name = attribute_type_name(attr, res);
        name.contains("HarmonyPostfix")
    }

    // =========================================================================
    // SPIKE FINDING: How the dotnetdll API works for custom attribute parsing
    // =========================================================================
    //
    // ## API Call Sequence
    //
    // 1. Parse DLL: `Resolution::parse(&bytes, ReadOptions::default())`
    // 2. Iterate types: `res.enumerate_type_definitions()` -> (TypeIndex, &TypeDefinition)
    // 3. Access type attributes: `typedef.attributes` -> Vec<Attribute>
    // 4. Identify attribute type: Match on `attr.constructor` (UserMethod::Reference for
    //    external attributes like Harmony). Get parent type name from the method reference.
    // 5. Decode attribute data: `attr.instantiation_data(&AlwaysFailsResolver, &res)`
    //    Returns `CustomAttributeData { constructor_args, named_args }`.
    //    AlwaysFailsResolver works as long as no enum values appear in the attribute args.
    // 6. Extract args: Pattern match on `FixedArg` variants:
    //    - `FixedArg::Type(Cow<str>)` for typeof() arguments
    //    - `FixedArg::String(Some(Cow<str>))` for string arguments
    //    - `FixedArg::Array(FieldOrPropType, Some(Vec<FixedArg>))` for Type[] arguments
    //
    // ## How typeof() is Encoded (FixedArg::Type)
    //
    // The `typeof(System.String)` argument appears as:
    //   FixedArg::Type("System.String, ...")
    //
    // The string is an ASSEMBLY-QUALIFIED type name, not a simple name.
    // For example: "System.String, netstandard, Version=2.0.0.0, ..."
    // For our purposes, we need to parse out just the type name portion
    // (everything before the first comma).
    //
    // Generic types use backtick notation, e.g.:
    //   "System.Collections.Generic.List`1[[System.String, ...]], netstandard, ..."
    //
    // ## Method-Level Attributes
    //
    // Method-level attributes ([HarmonyPrefix], [HarmonyPostfix]) are accessible via:
    //   typedef.methods[i].attributes -> Vec<Attribute>
    //
    // These work the same way as type-level attributes.
    //
    // ## HarmonyPatch Attribute Overloads
    //
    // The HarmonyPatch attribute has many constructor overloads:
    // - HarmonyPatch()  — no args
    // - HarmonyPatch(Type)  — target type only
    // - HarmonyPatch(Type, string)  — target type + method name
    // - HarmonyPatch(Type, string, Type[])  — target type + method name + parameter types
    // - HarmonyPatch(string)  — method name only (type specified separately)
    // - etc.
    //
    // Multiple [HarmonyPatch] attributes can be stacked on a single class to
    // specify different parts of the target (type, method, args) separately.
    //
    // ## Surprises / Limitations
    //
    // 1. AlwaysFailsResolver works for Harmony attributes since they don't use
    //    enum values in constructor args (HarmonyPatchType enum uses named args
    //    or a different attribute entirely).
    // 2. Type names in FixedArg::Type are assembly-qualified — need string parsing.
    // 3. The <Module> pseudo-type is always type_definitions[0] — skip it.
    // 4. Harmony Ref package (Lib.Harmony.Ref) is a reference-only assembly,
    //    so all Harmony types appear as ExternalTypeReferences, and attribute
    //    constructors appear as UserMethod::Reference.
    // 5. BUG in dotnetdll 0.2.0: SerString parser uses a single u8 for the
    //    string length instead of the ECMA-335 PackedLen (compressed unsigned
    //    int, 1/2/4 bytes). This means attribute arguments containing strings
    //    longer than ~127 bytes fail to parse. In practice this affects
    //    typeof() args for generic types with long assembly-qualified names
    //    (e.g. `List<string>` becomes ~200+ bytes). We must handle decode
    //    failures gracefully — the attribute is still identifiable by its
    //    constructor even when args fail to decode. Non-generic typeof() and
    //    all string/method-name arguments work fine.
    // =========================================================================

    #[test]
    fn spike_parse_harmony_attributes_from_test_dll() {
        // Step 1: Read the test DLL
        let dll_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/harmony/dlls/TestPatches.dll"
        );
        let bytes = std::fs::read(dll_path).expect("failed to read test DLL");

        // Step 2: Parse with dotnetdll
        let res = Resolution::parse(&bytes, ReadOptions::default())
            .expect("failed to parse DLL metadata");

        // =====================================================================
        // Validate: List all type definitions and their attributes
        // =====================================================================
        println!("=== Type Definitions ===");
        for (type_idx, typedef) in res.enumerate_type_definitions() {
            println!(
                "Type: {} (namespace: {:?}), {} attributes",
                typedef.name,
                typedef.namespace,
                typedef.attributes.len()
            );
            for attr in &typedef.attributes {
                let attr_type = attribute_type_name(attr, &res);
                println!("  Attribute: {}", attr_type);

                // Try to decode the attribute data
                match attr.instantiation_data(&AlwaysFailsResolver, &res) {
                    Ok(data) => {
                        println!("    Constructor args ({}):", data.constructor_args.len());
                        for (i, arg) in data.constructor_args.iter().enumerate() {
                            println!("      [{}] {:?}", i, arg);
                        }
                        if !data.named_args.is_empty() {
                            println!("    Named args ({}):", data.named_args.len());
                            for arg in &data.named_args {
                                println!("      {:?}", arg);
                            }
                        }
                    }
                    Err(e) => println!("    Failed to decode: {}", e),
                }
            }

            // Also print method-level attributes
            for (_method_idx, method) in res.enumerate_methods(type_idx) {
                if !method.attributes.is_empty() {
                    println!(
                        "  Method: {}, {} attributes",
                        method.name,
                        method.attributes.len()
                    );
                    for attr in &method.attributes {
                        let attr_type = attribute_type_name(attr, &res);
                        println!("    Attribute: {}", attr_type);
                    }
                }
            }
        }

        // =====================================================================
        // Validate SimplePatch: [HarmonyPatch(typeof(System.String), "Concat")]
        // =====================================================================
        println!("\n=== SimplePatch Validation ===");

        let simple_patch = res
            .enumerate_type_definitions()
            .find(|(_, td)| td.name == "SimplePatch")
            .expect("SimplePatch type not found");

        let simple_patch_typedef = simple_patch.1;
        assert!(
            !simple_patch_typedef.attributes.is_empty(),
            "SimplePatch should have attributes"
        );

        // Find the HarmonyPatch attribute
        let harmony_attr = simple_patch_typedef
            .attributes
            .iter()
            .find(|a| is_harmony_patch_attr(a, &res))
            .expect("SimplePatch should have a HarmonyPatch attribute");

        // Decode the attribute data
        let data = harmony_attr
            .instantiation_data(&AlwaysFailsResolver, &res)
            .expect("Failed to decode HarmonyPatch attribute data");

        // Verify constructor args
        assert_eq!(
            data.constructor_args.len(),
            2,
            "SimplePatch HarmonyPatch should have 2 constructor args (typeof + method name)"
        );

        // First arg: typeof(System.String) -> FixedArg::Type
        match &data.constructor_args[0] {
            FixedArg::Type(type_name) => {
                println!("  Target type (raw): {}", type_name);
                // Extract just the type name (before the first comma for assembly-qualified names)
                let simple_name = type_name.split(',').next().unwrap_or(type_name);
                assert_eq!(
                    simple_name, "System.String",
                    "Target type should be System.String"
                );
                println!("  Target type (parsed): {}", simple_name);
            }
            other => panic!("Expected FixedArg::Type for first arg, got: {:?}", other),
        }

        // Second arg: "Concat" -> FixedArg::String
        match &data.constructor_args[1] {
            FixedArg::String(Some(method_name)) => {
                assert_eq!(
                    method_name.as_ref(),
                    "Concat",
                    "Target method should be Concat"
                );
                println!("  Target method: {}", method_name);
            }
            other => panic!("Expected FixedArg::String for second arg, got: {:?}", other),
        }

        // =====================================================================
        // Validate method-level attributes: [HarmonyPrefix] and [HarmonyPostfix]
        // =====================================================================
        println!("\n=== Method-Level Attribute Validation ===");

        let prefix_method = simple_patch_typedef
            .methods
            .iter()
            .find(|m| m.name == "Prefix")
            .expect("SimplePatch should have a Prefix method");

        let has_prefix_attr = prefix_method
            .attributes
            .iter()
            .any(|a| is_harmony_prefix_attr(a, &res));
        assert!(
            has_prefix_attr,
            "Prefix method should have [HarmonyPrefix] attribute"
        );
        println!("  Prefix method has [HarmonyPrefix]: true");

        let postfix_method = simple_patch_typedef
            .methods
            .iter()
            .find(|m| m.name == "Postfix")
            .expect("SimplePatch should have a Postfix method");

        let has_postfix_attr = postfix_method
            .attributes
            .iter()
            .any(|a| is_harmony_postfix_attr(a, &res));
        assert!(
            has_postfix_attr,
            "Postfix method should have [HarmonyPostfix] attribute"
        );
        println!("  Postfix method has [HarmonyPostfix]: true");

        // =====================================================================
        // Validate SplitAttributePatch: multiple [HarmonyPatch] attributes
        // [HarmonyPatch(typeof(List<string>))]
        // [HarmonyPatch("Add")]
        //
        // KNOWN LIMITATION: The typeof(List<string>) attribute fails to decode
        // because the assembly-qualified generic type name is longer than what
        // dotnetdll's SerString parser can handle (it uses a single u8 for the
        // length instead of ECMA-335 PackedLen). We validate that:
        //   1. Both HarmonyPatch attributes are found on the type
        //   2. The generic-type attribute fails gracefully (we can still
        //      identify it as a HarmonyPatch by its constructor)
        //   3. The string-only attribute ("Add") decodes successfully
        // =====================================================================
        println!("\n=== SplitAttributePatch Validation ===");

        let split_patch = res
            .enumerate_type_definitions()
            .find(|(_, td)| td.name == "SplitAttributePatch")
            .expect("SplitAttributePatch type not found");

        let split_attrs: Vec<_> = split_patch
            .1
            .attributes
            .iter()
            .filter(|a| is_harmony_patch_attr(a, &res))
            .collect();

        assert_eq!(
            split_attrs.len(),
            2,
            "SplitAttributePatch should have 2 HarmonyPatch attributes"
        );

        // Decode both attributes — one will fail (generic type), one will succeed (string)
        let mut found_method_arg = false;
        let mut found_decode_failure = false;

        for attr in &split_attrs {
            match attr.instantiation_data(&AlwaysFailsResolver, &res) {
                Ok(data) => {
                    for arg in &data.constructor_args {
                        match arg {
                            FixedArg::Type(type_name) => {
                                let simple_name = type_name.split(',').next().unwrap_or(type_name);
                                println!("  Split attr type arg: {}", simple_name);
                            }
                            FixedArg::String(Some(s)) => {
                                println!("  Split attr string arg: {}", s);
                                assert_eq!(s.as_ref(), "Add", "Method name should be Add");
                                found_method_arg = true;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    // Expected: typeof(List<string>) produces a SerString longer than 255
                    // bytes, which overflows dotnetdll's single-byte length parser.
                    println!("  Expected decode failure for generic typeof() attr: {}", e);
                    found_decode_failure = true;
                }
            }
        }

        assert!(
            found_method_arg,
            "Should have found a method name arg in split attributes"
        );
        assert!(
            found_decode_failure,
            "Should have hit the known dotnetdll SerString bug for generic typeof()"
        );
        println!(
            "  KNOWN LIMITATION: generic typeof() args fail to decode (dotnetdll SerString bug)"
        );

        // =====================================================================
        // Validate OverloadPatch: Type[] parameter types
        // [HarmonyPatch(typeof(String), "Format", new Type[] { typeof(string), typeof(object) })]
        // =====================================================================
        println!("\n=== OverloadPatch Validation ===");

        let overload_patch = res
            .enumerate_type_definitions()
            .find(|(_, td)| td.name == "OverloadPatch")
            .expect("OverloadPatch type not found");

        let overload_attr = overload_patch
            .1
            .attributes
            .iter()
            .find(|a| is_harmony_patch_attr(a, &res))
            .expect("OverloadPatch should have a HarmonyPatch attribute");

        let data = overload_attr
            .instantiation_data(&AlwaysFailsResolver, &res)
            .expect("Failed to decode OverloadPatch attribute");

        assert_eq!(
            data.constructor_args.len(),
            3,
            "OverloadPatch should have 3 constructor args"
        );

        // Third arg: Type[] array
        match &data.constructor_args[2] {
            FixedArg::Array(elem_type, Some(elements)) => {
                println!(
                    "  Parameter types array: {} elements, elem_type: {:?}",
                    elements.len(),
                    elem_type
                );
                assert_eq!(elements.len(), 2, "Should have 2 parameter types");
                for (i, elem) in elements.iter().enumerate() {
                    match elem {
                        FixedArg::Type(type_name) => {
                            let simple_name = type_name.split(',').next().unwrap_or(type_name);
                            println!("    [{}] {}", i, simple_name);
                        }
                        other => println!("    [{}] unexpected: {:?}", i, other),
                    }
                }
            }
            other => panic!("Expected FixedArg::Array for third arg, got: {:?}", other),
        }

        // =====================================================================
        // Validate AnnotationOnlyPatch: [HarmonyPatch] with no args
        // =====================================================================
        println!("\n=== AnnotationOnlyPatch Validation ===");

        let anno_patch = res
            .enumerate_type_definitions()
            .find(|(_, td)| td.name == "AnnotationOnlyPatch")
            .expect("AnnotationOnlyPatch type not found");

        let anno_attr = anno_patch
            .1
            .attributes
            .iter()
            .find(|a| is_harmony_patch_attr(a, &res))
            .expect("AnnotationOnlyPatch should have a HarmonyPatch attribute");

        let data = anno_attr
            .instantiation_data(&AlwaysFailsResolver, &res)
            .expect("Failed to decode AnnotationOnlyPatch attribute");

        assert_eq!(
            data.constructor_args.len(),
            0,
            "AnnotationOnlyPatch should have 0 constructor args"
        );
        println!("  No-arg HarmonyPatch attribute decoded successfully");

        // =====================================================================
        // SPIKE CONCLUSION
        // =====================================================================
        println!("\n=== SPIKE RESULTS ===");
        println!("dotnetdll 0.2.0 CAN parse Harmony [HarmonyPatch] attributes.");
        println!("Validated attribute forms:");
        println!("  [PASS] typeof(non-generic) + method name constructor");
        println!("  [PASS] Stacked/split [HarmonyPatch] attributes");
        println!("  [PASS] Type[] parameter type arrays");
        println!("  [PASS] No-arg [HarmonyPatch]");
        println!("  [PASS] Method-level [HarmonyPrefix] and [HarmonyPostfix]");
        println!("  [FAIL] typeof(generic<T>) — dotnetdll SerString bug for long type names");
        println!();
        println!("VERDICT: Approach is VIABLE for Harmony conflict detection.");
        println!("The generic typeof() limitation is non-blocking:");
        println!("  - Most real Harmony patches target non-generic types");
        println!("  - We can still identify the *attribute* even when args fail to decode");
        println!("  - A future dotnetdll fix or workaround can address this edge case");
    }
}
