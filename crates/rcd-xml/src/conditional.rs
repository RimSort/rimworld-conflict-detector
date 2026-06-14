// Conditional patch operation handling for RimWorld mods
//
// PatchOperationFindMod and PatchOperationConditional support is integrated
// directly into `patch_ops.rs` since the conditional parsing is interleaved
// with the main operation parsing logic. This module re-exports the relevant
// types for ergonomic access.

pub use crate::patch_ops::PatchCondition;
