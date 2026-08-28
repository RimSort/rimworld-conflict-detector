pub mod about_xml;
pub mod conflict;
pub mod detector;
pub mod diagnostic;
pub mod folder;
pub mod load_order;
pub mod mods_config;
pub mod report;
pub mod severity;
pub mod types;

#[cfg(any(test, feature = "test-helpers"))]
pub mod testutil;

pub use conflict::{
    Conflict, DefConflict, DefConflictMechanism, DefTarget, HarmonyConflict, HarmonyPatchInfo,
    HarmonyPatchKind, HarmonyTarget,
};
pub use detector::{Detector, Engine, ProgressFn};
pub use diagnostic::{Diagnostic, DiagnosticSource, about_parse_failure};
pub use report::Report;
pub use severity::Severity;
pub use types::{LoadOrder, Mod, ModSummary};
