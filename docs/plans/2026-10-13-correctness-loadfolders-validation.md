# Correctness and LoadFolders Sprint Validation

## Automated gates

Final source revision: `1ad7261` (subsequent commits only document the work and remove tracked scratch artifacts).

- `just ci`: passed; 216 tests passed, one pre-existing test ignored, zero failures. Includes workspace tests, Clippy, formatting, and workspace build.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- Task-scoped independent reviews completed for all implementation tasks; identified issues received fixes and scoped re-review.
- Whole-branch review and scoped final fix review completed. Attribute entity canonicalization and directory-inspection diagnostic fixes were verified. The original malformed-end-tag example was already rejected by quick-xml's default end-name checking; the added explicit stack is defensive validation, not a reproduced fix for that example.

Some new API tests have passing behavioral assertions but lack pre-implementation behavioral RED evidence. This is a test-process limitation, not proof of a runtime defect. The reports explicitly distinguish those cases from assertion-level RED/GREEN regressions.

## Real workshop corpus

Both revisions scanned the same 1,566 discovered mods, with all discovered mods considered active by the approved directory-input policy. This is not a curated compatible load order and does not include official base/DLC content unless present among the supplied inputs.

| Run | Baseline `d3dbd89` | Final source `1ad7261` |
|---|---:|---:|
| No game version: total conflicts | 88,355 | 88,301* |
| Game version 1.7: total conflicts | 107,997 | 3,501,477 |
| 1.7 Error findings | 79 | 944 |
| 1.7 Warning findings | 974 | 11,939 |
| 1.7 Info findings | 106,944 | 3,488,594 |
| 1.7 mods with conflicts | 334 | 745 |

*The no-version comparison was run before the final attribute/discovery validation fixes. The explicit-1.7 corpus run was repeated on the final source; its totals remained unchanged.*

The final 1.7 report has 481 direct replacements, 3,495,600 existing XPath-overlap findings, 199 XPath-on-replaced-def findings, 4,118 new same-def non-overlap aggregates, and 1,079 Harmony findings. Most growth is in existing XPath overlap reporting, not the new aggregate mechanism.

The versioned diagnostic analysis found one FolderResolution Warning (`airetexture.core`, no selected folders), one FolderResolution Info (`ferny.betterarchitect`, unknown `IfModActiveAny`), 310 XML Info diagnostics, and 75,357 DLL Info diagnostics. The final scan returned exit 1 for conflict findings, with no stderr output. Its JSON is approximately 3.06 GB uncompressed / 115 MB gzip; generating and consuming reports at this scale remains a practical output-volume limitation.

## Representative source traces

Streaming comparison found 3,387,845 added XPath-overlap rows relative to baseline signatures. The largest newly reported pair was `ferny.betterarchitect` and `ferny.progressionaesthetics` (126,735 rows); the next was Better Architect and `ferny.progressionattire` (94,404 rows).

- Better Architect's selected v1.6 section contains 1,072 entries. A 1.7 request uses this greatest-lower section. Its `Mods and Shit/Ancient Urban Ruins` entry requires `xmb.ancienturbanruins.mo`, which is present in the discovered corpus. Progression: Aesthetics also selects its `1.6/Mods and Shit/Ancient Urban Ruins` entry behind the same satisfied guard. Both selected patch files contain XPath operations targeting `AM_FLOOR`, explaining a representative added conflict between these mods.
- Better Architect selects `Mods and Shit/Mythic Ages Megafauna Bestiary` behind the satisfied `veterano.mythicages.megafaunabestiary` guard. Its patch changes `ThingDef[defName="MA_FlatButcherBlock"]/designationCategory`. Progression: Attire selects `Mods/Biomes! Fossils` behind the satisfied `BiomesTeam.BiomesFossils` guard; that patch targets `ThingDef[@Name="BMT_Apparel_DinoHatCarni_Base"]`. These selected files explain a representative wildcard Info pairing. The existing coarse XPath model treats unsupported name selectors conservatively; this is not proof the two runtime XML nodes actually intersect.

These traces establish recovered conditional/versioned content for representative additions. They do not establish a corpus-wide false-positive rate or validate every newly reported pair. Existing wildcard approximation and the all-installed-mods-active policy can produce very large cross-products.

## Remaining limitations

- `IfModActiveAny` is ignored as an unknown attribute and reported once per mod, as explicitly specified in the approved design. This is not an additional supported condition.
- `PatchOperationSequence.success` is not evaluated; contained operations are conservatively analyzed.
- The corpus audit is sampled, not exhaustive. Counts alone cannot distinguish every real incompatibility from a conservative static-analysis finding.
- No push or merge was performed. The feature branch is retained for user review.

## Post-review fix validation

A subsequent Qwen3.8 Max review of `5d616bc` identified nine actionable findings. Parent verification reproduced the five behavioral findings and confirmed the four cleanup/documentation findings from source. The fix set adds 11 regression tests covering nested self-closing folder markup, retained invalid-path details, active-only config folder diagnostics (including the exit-code regression and preservation of missing-active warnings), root-attribute field ordering, and padded CDATA XPath normalization.

The fix set passed a fresh `just ci` and `cargo clippy --workspace --all-targets -- -D warnings`: **227 passed, one pre-existing ignored, zero failures**, with formatting and workspace build passing. Behavioral assertion-level RED/GREEN evidence was recorded for the bug fixes; the pre-fix randomized attribute-order test failed in five separate invocations, with 64 independent comparisons per invocation. Dead-code/dependency cleanup and documentation updates do not have behavioral RED assertions.

An independent Qwen3.8 Max re-review verified the code fixes and regression-test discrimination. A scoped follow-up verified the corrected documentation; all nine findings were closed with no remaining findings in that review.

The corpus figures above predate these review fixes. No new full-corpus scan was performed for the fix set; those earlier figures must not be represented as a scan of the post-review source. The existing sampled-audit and output-volume limitations remain unchanged.
