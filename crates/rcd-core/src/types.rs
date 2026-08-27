use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mod {
    pub package_id: String,
    pub name: String,
    pub author: Option<String>,
    pub supported_versions: Vec<String>,
    pub dependencies: Vec<String>,
    pub incompatibilities: Vec<String>,
    pub load_after: Vec<String>,
    pub load_before: Vec<String>,
    pub force_load_after: Vec<String>,
    pub force_load_before: Vec<String>,
    pub is_base_content: bool,
    pub root_path: PathBuf,
    pub defs_paths: Vec<PathBuf>,
    pub patches_paths: Vec<PathBuf>,
    pub assemblies_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModSummary {
    pub package_id: String,
    pub name: String,
    pub author: Option<String>,
    pub is_base_content: bool,
}

impl From<&Mod> for ModSummary {
    fn from(m: &Mod) -> Self {
        ModSummary {
            package_id: m.package_id.clone(),
            name: m.name.clone(),
            author: m.author.clone(),
            is_base_content: m.is_base_content,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadOrder {
    pub package_ids: Vec<String>,
    positions: HashMap<String, usize>,
}

impl LoadOrder {
    pub fn new(package_ids: Vec<String>) -> Self {
        let mut positions = HashMap::with_capacity(package_ids.len());
        for (i, id) in package_ids.iter().enumerate() {
            // First occurrence wins, matching the old Vec::position behavior.
            positions.entry(id.clone()).or_insert(i);
        }
        Self {
            package_ids,
            positions,
        }
    }

    pub fn position(&self, package_id: &str) -> Option<usize> {
        self.positions.get(package_id).copied()
    }

    pub fn is_after(&self, a: &str, b: &str) -> Option<bool> {
        match (self.position(a), self.position(b)) {
            (Some(pa), Some(pb)) => Some(pa > pb),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_order_position() {
        let order = LoadOrder::new(vec!["core".into(), "mod.a".into(), "mod.b".into()]);
        assert_eq!(order.position("core"), Some(0));
        assert_eq!(order.position("mod.b"), Some(2));
        assert_eq!(order.position("unknown"), None);
    }

    #[test]
    fn load_order_duplicate_package_id_first_occurrence_wins() {
        let order = LoadOrder::new(vec!["core".into(), "mod.a".into(), "mod.a".into()]);
        assert_eq!(order.position("mod.a"), Some(1), "must match Vec::position semantics");
    }

    #[test]
    fn load_order_position_matches_vec_semantics() {
        let ids = vec!["a".into(), "b".into(), "c".into()];
        let order = LoadOrder::new(ids.clone());
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(order.position(id), Some(i));
        }
        assert_eq!(order.position("missing"), None);
    }

    #[test]
    fn load_order_is_after() {
        let order = LoadOrder::new(vec!["core".into(), "mod.a".into(), "mod.b".into()]);
        assert_eq!(order.is_after("mod.b", "mod.a"), Some(true));
        assert_eq!(order.is_after("mod.a", "mod.b"), Some(false));
        assert_eq!(order.is_after("mod.a", "unknown"), None);
    }
}
