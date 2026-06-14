use crate::report::ReportMetadata;
use crate::{Conflict, Diagnostic, LoadOrder, Mod, Report};

pub trait Detector: Send + Sync {
    fn name(&self) -> &str;
    fn analyze(&self, mods: &[Mod], order: Option<&LoadOrder>) -> (Vec<Conflict>, Vec<Diagnostic>);
}

pub type ProgressFn = Box<dyn Fn(u32, u32, &str) + Send + Sync>;

pub struct Engine {
    detectors: Vec<Box<dyn Detector>>,
}

impl Engine {
    pub fn new() -> Self {
        Engine { detectors: vec![] }
    }

    pub fn add_detector(&mut self, detector: Box<dyn Detector>) {
        self.detectors.push(detector);
    }

    pub fn analyze(
        &self,
        mods: &[Mod],
        order: Option<&LoadOrder>,
        metadata: ReportMetadata,
        progress: Option<&ProgressFn>,
    ) -> Report {
        let mut all_conflicts = Vec::new();
        let mut all_diagnostics = Vec::new();

        for (i, detector) in self.detectors.iter().enumerate() {
            if let Some(progress) = progress {
                progress(i as u32, self.detectors.len() as u32, detector.name());
            }

            let (conflicts, diagnostics) = detector.analyze(mods, order);
            all_conflicts.extend(conflicts);
            all_diagnostics.extend(diagnostics);
        }

        Report::build(metadata, all_conflicts, all_diagnostics)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModSummary;
    use crate::conflict::*;

    struct FakeDetector {
        conflicts: Vec<Conflict>,
    }

    impl Detector for FakeDetector {
        fn name(&self) -> &str {
            "fake"
        }

        fn analyze(
            &self,
            _mods: &[Mod],
            _order: Option<&LoadOrder>,
        ) -> (Vec<Conflict>, Vec<Diagnostic>) {
            (self.conflicts.clone(), vec![])
        }
    }

    #[test]
    fn engine_merges_detector_results() {
        let mut engine = Engine::new();
        engine.add_detector(Box::new(FakeDetector {
            conflicts: vec![Conflict::Def(DefConflict {
                target: DefTarget {
                    def_type: "ThingDef".into(),
                    def_name: "A".into(),
                },
                mods: vec![ModSummary {
                    package_id: "mod.a".into(),
                    name: "Mod A".into(),
                    author: None,
                    is_base_content: false,
                }],
                mechanism: DefConflictMechanism::DirectReplacement {
                    divergent_fields: vec!["label".into()],
                },
                winner: None,
                condition: None,
            })],
        }));

        let metadata = ReportMetadata {
            timestamp: "2026-06-14T00:00:00Z".into(),
            game_version: None,
            order_aware: false,
            mods_scanned: vec![],
        };

        let report = engine.analyze(&[], None, metadata, None);
        assert_eq!(report.conflicts.len(), 1);
    }
}
