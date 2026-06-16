use crate::{Diagnostic, DiagnosticSource, LoadOrder, Mod, Severity};

pub fn validate_load_order(mods: &[Mod], order: &LoadOrder) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for m in mods {
        let my_pos = match order.position(&m.package_id) {
            Some(p) => p,
            None => continue,
        };

        // loadAfter: this mod should load AFTER the listed mods
        for dep in &m.load_after {
            if let Some(dep_pos) = order.position(dep)
                && my_pos < dep_pos
            {
                diagnostics.push(Diagnostic {
                    affected_mod: Some(m.package_id.clone()),
                    severity: Severity::Warning,
                    message: format!(
                        "'{}' declares loadAfter '{}' but is loaded before it",
                        m.package_id, dep
                    ),
                    source: DiagnosticSource::MetadataResolution,
                });
            }
        }

        // loadBefore: this mod should load BEFORE the listed mods
        for dep in &m.load_before {
            if let Some(dep_pos) = order.position(dep)
                && my_pos > dep_pos
            {
                diagnostics.push(Diagnostic {
                    affected_mod: Some(m.package_id.clone()),
                    severity: Severity::Warning,
                    message: format!(
                        "'{}' declares loadBefore '{}' but is loaded after it",
                        m.package_id, dep
                    ),
                    source: DiagnosticSource::MetadataResolution,
                });
            }
        }

        // forceLoadAfter: hard constraint — use Error severity
        for dep in &m.force_load_after {
            if let Some(dep_pos) = order.position(dep)
                && my_pos < dep_pos
            {
                diagnostics.push(Diagnostic {
                    affected_mod: Some(m.package_id.clone()),
                    severity: Severity::Error,
                    message: format!(
                        "'{}' declares forceLoadAfter '{}' but is loaded before it (hard constraint violated)",
                        m.package_id, dep
                    ),
                    source: DiagnosticSource::MetadataResolution,
                });
            }
        }

        // forceLoadBefore: hard constraint — use Error severity
        for dep in &m.force_load_before {
            if let Some(dep_pos) = order.position(dep)
                && my_pos > dep_pos
            {
                diagnostics.push(Diagnostic {
                    affected_mod: Some(m.package_id.clone()),
                    severity: Severity::Error,
                    message: format!(
                        "'{}' declares forceLoadBefore '{}' but is loaded after it (hard constraint violated)",
                        m.package_id, dep
                    ),
                    source: DiagnosticSource::MetadataResolution,
                });
            }
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_mod(id: &str, load_after: Vec<&str>, load_before: Vec<&str>) -> Mod {
        Mod {
            package_id: id.to_string(),
            name: id.to_string(),
            author: None,
            supported_versions: vec![],
            dependencies: vec![],
            incompatibilities: vec![],
            load_after: load_after.into_iter().map(String::from).collect(),
            load_before: load_before.into_iter().map(String::from).collect(),
            force_load_after: vec![],
            force_load_before: vec![],
            is_base_content: false,
            root_path: PathBuf::new(),
            defs_paths: vec![],
            patches_paths: vec![],
            assemblies_paths: vec![],
        }
    }

    #[test]
    fn load_after_violation() {
        let mods = vec![
            test_mod("mod.a", vec!["mod.b"], vec![]),
            test_mod("mod.b", vec![], vec![]),
        ];
        let order = LoadOrder {
            package_ids: vec!["mod.a".into(), "mod.b".into()],
        };
        // mod.a says loadAfter mod.b, but mod.a is position 0 and mod.b is position 1
        let diags = validate_load_order(&mods, &order);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
        assert!(diags[0].message.contains("loadAfter"));
    }

    #[test]
    fn correct_order_no_diagnostic() {
        let mods = vec![
            test_mod("mod.a", vec![], vec![]),
            test_mod("mod.b", vec!["mod.a"], vec![]),
        ];
        let order = LoadOrder {
            package_ids: vec!["mod.a".into(), "mod.b".into()],
        };
        // mod.b says loadAfter mod.a, and it IS after — correct
        let diags = validate_load_order(&mods, &order);
        assert!(diags.is_empty());
    }

    #[test]
    fn load_before_violation() {
        let mods = vec![
            test_mod("mod.a", vec![], vec![]),
            test_mod("mod.b", vec![], vec!["mod.a"]),
        ];
        let order = LoadOrder {
            package_ids: vec!["mod.a".into(), "mod.b".into()],
        };
        // mod.b says loadBefore mod.a, but mod.b is at position 1 (after mod.a at 0)
        let diags = validate_load_order(&mods, &order);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("loadBefore"));
    }
}
