use crate::{ModSummary, Severity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefTarget {
    pub def_type: String,
    pub def_name: String,
}

impl std::fmt::Display for DefTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.def_type, self.def_name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefConflictMechanism {
    DirectReplacement {
        divergent_fields: Vec<String>,
    },
    XPathOverlap {
        xpath_a: String,
        xpath_b: String,
        wildcard: bool,
    },
    XPathOnReplacedDef {
        xpath: String,
        replaced_by: String,
    },
    #[serde(rename = "xpath_same_def_non_overlap")]
    XPathSameDefNonOverlap {
        xpath_pairs: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefConflict {
    pub target: DefTarget,
    pub mods: Vec<ModSummary>,
    pub mechanism: DefConflictMechanism,
    pub winner: Option<String>,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarmonyPatchKind {
    Prefix,
    Postfix,
    Transpiler,
    Finalizer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarmonyPatchInfo {
    pub mod_summary: ModSummary,
    pub dll_name: String,
    pub patch_kind: HarmonyPatchKind,
    pub priority: i32,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub returns_bool: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarmonyTarget {
    pub type_name: String,
    pub method_name: String,
    pub parameter_types: Option<Vec<String>>,
}

impl std::fmt::Display for HarmonyTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", self.type_name, self.method_name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarmonyConflict {
    pub target: HarmonyTarget,
    pub patches: Vec<HarmonyPatchInfo>,
    pub short_circuit_risk: bool,
    pub execution_order: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Conflict {
    Def(DefConflict),
    Harmony(HarmonyConflict),
}

impl Serialize for Conflict {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Repr<'a> {
            Def {
                #[serde(flatten)]
                payload: &'a DefConflict,
                severity: Severity,
            },
            Harmony {
                #[serde(flatten)]
                payload: &'a HarmonyConflict,
                severity: Severity,
            },
        }
        let severity = self.severity();
        match self {
            Self::Def(payload) => Repr::Def { payload, severity },
            Self::Harmony(payload) => Repr::Harmony { payload, severity },
        }
        .serialize(serializer)
    }
}

impl Conflict {
    pub fn severity(&self) -> Severity {
        match self {
            Conflict::Def(dc) => match &dc.mechanism {
                DefConflictMechanism::DirectReplacement { .. } => Severity::Error,
                DefConflictMechanism::XPathOverlap { wildcard, .. } => {
                    if *wildcard {
                        Severity::Info
                    } else {
                        Severity::Warning
                    }
                }
                DefConflictMechanism::XPathOnReplacedDef { .. } => Severity::Error,
                DefConflictMechanism::XPathSameDefNonOverlap { .. } => Severity::Info,
            },
            Conflict::Harmony(hc) => {
                let kinds: Vec<_> = hc.patches.iter().map(|p| p.patch_kind).collect();
                let has_prefix = kinds.contains(&HarmonyPatchKind::Prefix);
                let has_transpiler = kinds.contains(&HarmonyPatchKind::Transpiler);

                let mut transpiler_mods: Vec<&str> = hc
                    .patches
                    .iter()
                    .filter(|p| p.patch_kind == HarmonyPatchKind::Transpiler)
                    .map(|p| p.mod_summary.package_id.as_str())
                    .collect();
                transpiler_mods.sort_unstable();
                transpiler_mods.dedup();
                let has_multi_mod_transpilers = transpiler_mods.len() >= 2;

                if hc.short_circuit_risk {
                    Severity::Error
                } else if (has_prefix && has_transpiler) || has_multi_mod_transpilers {
                    Severity::Warning
                } else {
                    Severity::Info
                }
            }
        }
    }

    pub fn involved_mods(&self) -> Vec<&ModSummary> {
        match self {
            Conflict::Def(dc) => dc.mods.iter().collect(),
            Conflict::Harmony(hc) => hc.patches.iter().map(|p| &p.mod_summary).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::test_mod_summary;

    #[test]
    fn serialized_conflict_includes_derived_severity() {
        let conflict = Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
            },
            mods: vec![],
            mechanism: DefConflictMechanism::DirectReplacement {
                divergent_fields: vec![],
            },
            winner: None,
            condition: None,
        });
        let value = serde_json::to_value(&conflict).unwrap();
        assert_eq!(value["type"], "def");
        assert_eq!(value["severity"], "error");
        let decoded: Conflict = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.severity(), Severity::Error);
    }

    #[test]
    fn def_replacement_is_error() {
        let c = Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
            },
            mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
            mechanism: DefConflictMechanism::DirectReplacement {
                divergent_fields: vec!["statBases/MaxHitPoints".into()],
            },
            winner: None,
            condition: None,
        });
        assert_eq!(c.severity(), Severity::Error);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn xpath_overlap_is_warning() {
        let c = Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
            },
            mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
            mechanism: DefConflictMechanism::XPathOverlap {
                xpath_a: "Defs/ThingDef[defName=\"Steel\"]/statBases".into(),
                xpath_b: "Defs/ThingDef[defName=\"Steel\"]/statBases".into(),
                wildcard: false,
            },
            winner: None,
            condition: None,
        });
        assert_eq!(c.severity(), Severity::Warning);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn wildcard_xpath_overlap_is_info() {
        let c = Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: "ThingDef".into(),
                def_name: String::new(),
            },
            mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
            mechanism: DefConflictMechanism::XPathOverlap {
                xpath_a: "Defs/ThingDef/statBases".into(),
                xpath_b: "Defs/ThingDef[defName=\"Steel\"]/statBases".into(),
                wildcard: true,
            },
            winner: None,
            condition: None,
        });
        assert_eq!(c.severity(), Severity::Info);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn xpath_on_replaced_def_is_error() {
        let c = Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
            },
            mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
            mechanism: DefConflictMechanism::XPathOnReplacedDef {
                xpath: "Defs/ThingDef[defName=\"Steel\"]/statBases".into(),
                replaced_by: "mod.a".into(),
            },
            winner: None,
            condition: None,
        });
        assert_eq!(c.severity(), Severity::Error);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn same_def_non_overlapping_xpaths_are_info() {
        let c = Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
            },
            mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
            mechanism: DefConflictMechanism::XPathSameDefNonOverlap {
                xpath_pairs: vec![("path/a".into(), "path/b".into())],
            },
            winner: None,
            condition: None,
        });
        assert_eq!(c.severity(), Severity::Info);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn harmony_prefix_bool_is_error() {
        let c = Conflict::Harmony(HarmonyConflict {
            target: HarmonyTarget {
                type_name: "Pawn".into(),
                method_name: "Kill".into(),
                parameter_types: None,
            },
            patches: vec![
                HarmonyPatchInfo {
                    mod_summary: test_mod_summary("mod.a"),
                    dll_name: "ModA.dll".into(),
                    patch_kind: HarmonyPatchKind::Prefix,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: true,
                },
                HarmonyPatchInfo {
                    mod_summary: test_mod_summary("mod.b"),
                    dll_name: "ModB.dll".into(),
                    patch_kind: HarmonyPatchKind::Prefix,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: true,
                },
            ],
            short_circuit_risk: true,
            execution_order: None,
        });
        assert_eq!(c.severity(), Severity::Error);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn harmony_postfix_only_is_info() {
        let c = Conflict::Harmony(HarmonyConflict {
            target: HarmonyTarget {
                type_name: "Pawn".into(),
                method_name: "GetGizmos".into(),
                parameter_types: None,
            },
            patches: vec![
                HarmonyPatchInfo {
                    mod_summary: test_mod_summary("mod.a"),
                    dll_name: "ModA.dll".into(),
                    patch_kind: HarmonyPatchKind::Postfix,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: false,
                },
                HarmonyPatchInfo {
                    mod_summary: test_mod_summary("mod.b"),
                    dll_name: "ModB.dll".into(),
                    patch_kind: HarmonyPatchKind::Postfix,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: false,
                },
            ],
            short_circuit_risk: false,
            execution_order: None,
        });
        assert_eq!(c.severity(), Severity::Info);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn harmony_prefix_transpiler_is_warning() {
        let c = Conflict::Harmony(HarmonyConflict {
            target: HarmonyTarget {
                type_name: "Pawn".into(),
                method_name: "Kill".into(),
                parameter_types: None,
            },
            patches: vec![
                HarmonyPatchInfo {
                    mod_summary: test_mod_summary("mod.a"),
                    dll_name: "ModA.dll".into(),
                    patch_kind: HarmonyPatchKind::Prefix,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: false,
                },
                HarmonyPatchInfo {
                    mod_summary: test_mod_summary("mod.b"),
                    dll_name: "ModB.dll".into(),
                    patch_kind: HarmonyPatchKind::Transpiler,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: false,
                },
            ],
            short_circuit_risk: false,
            execution_order: None,
        });
        assert_eq!(c.severity(), Severity::Warning);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn harmony_two_transpilers_is_warning() {
        let c = Conflict::Harmony(HarmonyConflict {
            target: HarmonyTarget {
                type_name: "Pawn".into(),
                method_name: "Kill".into(),
                parameter_types: None,
            },
            patches: vec![
                HarmonyPatchInfo {
                    mod_summary: test_mod_summary("mod.a"),
                    dll_name: "ModA.dll".into(),
                    patch_kind: HarmonyPatchKind::Transpiler,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: false,
                },
                HarmonyPatchInfo {
                    mod_summary: test_mod_summary("mod.b"),
                    dll_name: "ModB.dll".into(),
                    patch_kind: HarmonyPatchKind::Transpiler,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: false,
                },
            ],
            short_circuit_risk: false,
            execution_order: None,
        });
        assert_eq!(c.severity(), Severity::Warning);
        assert_eq!(
            serde_json::to_value(&c).unwrap()["severity"],
            serde_json::to_value(c.severity()).unwrap()
        );
    }

    #[test]
    fn conflict_json_roundtrip() {
        let c = Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: "ThingDef".into(),
                def_name: "Steel".into(),
            },
            mods: vec![test_mod_summary("mod.a")],
            mechanism: DefConflictMechanism::DirectReplacement {
                divergent_fields: vec!["label".into()],
            },
            winner: Some("mod.a".into()),
            condition: None,
        });
        let json = serde_json::to_string(&c).unwrap();
        let parsed: Conflict = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.severity(), Severity::Error);
    }
}
