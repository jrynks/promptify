"""Publish explicit, independently assigned grades; never infer quality from reviewer verdicts."""
import json
from collections import Counter, defaultdict
from math import ceil
from pathlib import Path

DIMENSIONS = ["fidelity", "dependencies", "correction_recheck", "completion", "proportionality"]

# Each five-digit vector is a separate reading of one final candidate, not a model score.
BASELINE = {
    "o01": ("54323 00000 00000", "55554 55554 55555", "Legacy verifies an explanation step rather than the final explanation; two failures. Adaptive is accurate but adds definitions/examples."),
    "o02": ("34433 24433 34443", "55544 24433 34443", "Legacy #1/#2 test single clicks only; #2 approves one request per click event. #3 and adaptive #3 retain double-click testing but contradict it with per-click fixes. Adaptive #1 preserves the actual trigger."),
    "o03": ("00000 54333 54333", "55344 55344 55344", "Legacy copies pricing-example recommendation/table scaffolding. Adaptive lacks a distinct final verification step; uncertainty flags follow the loop target."),
    "o04": ("55554 55443 55544", "55444 55555 55554", "Faithful exclusions. Legacy #2 requires unnecessary details and leaks system prose; adaptive #1 asks optional missing-detail questions."),
    "o05": ("54443 54443 54543", "00000 54333 54332", "Mostly faithful separate evidence, but false cross-source prerequisites and added formats remain. Adaptive finalizes after verification and #3 is overbuilt."),
    "o06": ("14433 24433 55555", "55554 55555 55554", "Legacy #1 restores retracted email instead of French translation; #2 does both. #3 obtains the missing source. Adaptive preserves translation."),
    "o07": ("55313 55444 55344", "55444 54222 55544", "Legacy #1 says done at limit without success. Adaptive #2 appends more work after Done and treats maximum rounds as completion."),
    "o08": ("55443 55544 55443", "55555 55554 55555", "Legacy unnecessary third steps/clarification. Adaptive preserves gentle/no-scary/no-count request."),
    "o09": ("55444 55444 55343", "55534 55524 55555", "Legacy copies browser/visual-field boilerplate. Adaptive #1/#2 permit done on inability or round limit; #3 is faithful."),
    "h01": ("24322 14322 55543", "14322 14332 55544", "Several approved candidates replace actual summary/comparison/citation with requesting the missing report and mark that done. #3 in both retains work after source receipt."),
    "h02": ("55544 54334 55554", "55555 55555 55555", "Legacy #2 requires sending and receipt but only verifies content, not delivery; asking by email does not explicitly prohibit sending. Adaptive remains draft-oriented."),
    "h03": ("34333 34333 44423", "34434 55544 44434", "Legacy drops no-new-endpoint throughout and adds unnecessary questions; #3 incoherent multiple Done. Adaptive #1/#3 also omit endpoint exclusion; #2 preserves trigger/exclusion."),
    "h04": ("34423 44533 34423", "55554 55554 55554", "Legacy invents session/question limits or timeout and repetitive workflow. Adaptive checks current turn and waits without invented total."),
    "h05": ("55555 55554 55444", "55554 55554 55555", "French translation with names/date/amount intact. Some unnecessary clarification, literal structure, and final output steps."),
    "h06": ("55544 55333 55443", "55544 55544 55443", "All preserve 120 vs 102 attribution and no resolution. Legacy #2 checks earlier disagreement not final comparison; #3 malformed avoid-averaging loop condition."),
    "h07": ("55233 45232 45232", "55233 55232 55243", "Initial tests rerun after edit, but correction loop reruns only lint, not tests after its new edit. Some Done conditions ignore test failures."),
    "h08": ("55544 55554 55555", "55555 55554 55323", "Actual image generally retained. Adaptive #3 permits completion at limit and checks only excluded text."),
    "b1_h09": ("55443 55443 55343", "00000 00000 00000", "Legacy preserves separate evidence but substitutes missing-data loops for final evidence verification. Adaptive has three failures."),
    "b1_h10": ("55544 55444 55544", "55443 55544 55555", "Planned May 8 and confirmation after testing remain; redundant finalization and system boilerplate in adaptive #1."),
    "b2_h11": ("55544 55543 55543", "55544 55343 55444", "Prototype/pending review/undecided launch preserved. Adaptive #2 loops to asking for owner, not erroneous status text; some unnecessary questions."),
    "b2_h12": ("55544 35443 55544", "35433 45544 35433", "Legacy #2 loses the quoted message. Adaptive #1/#3 ban quoting command text even as summary and #1 invents a contradiction requiring missing report."),
    "b3_h13": ("55443 45544 45443", "35443 55543 55444", "Legacy unnecessary contact details and invented Organizer salutation. Adaptive #1 states unconfirmed venue but omits asking organizer to confirm it."),
    "b3_h14": ("55444 55444 45433", "55544 00000 55544", "Regression starts non-default and reloads. Legacy #3 omits unchanged key/default exclusion and checks tests before final verification; adaptive #2 failed."),
    "b4_h15": ("55544 55543 55443", "55343 55343 55343", "Both quotes and seat count retained; extra format/scaffolding. Adaptive corrects attribution instead of deficient evidence checklist/recommendation."),
    "b4_h16": ("45433 55232 55243", "55544 55544 55544", "Legacy #1 omits no-dependency; #2 repairs test rather than implementation; #3 rechecks manual behavior without rerunning regression after correction."),
    "b5_h17": ("55544 45433 55332", "45433 55544 55544", "Draft/investigation/review generally retained. Legacy #2 adds mandatory details; #3 bloats eight steps and edits after loop check. Adaptive #1 invents referenced thread."),
    "b5_h18": ("55544 55544 55343", "55343 55233 00000", "Attribution and unresolved status kept; adaptive loop checks earlier summary or unrelated source instead of final question and composition."),
    "b6_h19": ("55555 55555 55544", "55555 55544 55555", "One-sentence/no-code/no-analogy intact; some checks omit sentence count."),
    "b6_h20": ("55543 55343 55332", "00000 55555 00000", "Internal beta status retained in usable samples; legacy adds questions, fragmented steps, finalization after check. Adaptive #1/#3 fail."),
    "b7_h21": ("55543 55543 55544", "55242 55544 55544", "Confirmed vs estimates/two undecided locations/no recommendation retained; legacy invented formats, adaptive #1 repairs only recommendation not dates/status."),
    "b7_h22": ("55343 45333 35433", "55555 55555 55555", "Legacy fragmented checks; #2 invents no receipt condition and #3 drops updated-invite condition from drafting action. Adaptive faithfully carries conditional timing."),
}

ROUND_ONE = {
    "o01": ("55343 55343 55343", "55554 55554 55554", "Default expands a simple explanation into several production steps and uses synthesis as verification. Adaptive stays concise."),
    "o02": ("45544 35443 55554", "25424 25424 25423", "Default #1 drops API exclusion; #2 ambiguous per-click action remains. Adaptive keeps per-click success/fix, contradicting the double-click trigger; #1 is reviewer-approved despite surface block."),
    "o03": ("45343 45343 45342", "55543 00000 55544", "Default copies unrequested recommendation, even after removing table example; #3 overbuilt. Adaptive #2 failed."),
    "o04": ("00000 55554 55555", "55555 45544 55555", "Default #1 failed; adaptive #2 invents a recipient placeholder. Other drafts preserve no apology/deadline."),
    "o05": ("54544 00000 54543", "54542 54544 54544", "False dependency between independent source analyses remains; default #2 failed; adaptive #1 has nine steps and meta-response."),
    "o06": ("14433 04422 04422", "55554 55554 55555", "All default samples restore the retracted email; #2 forbids translation, #3 invents apology/email translation. Adaptive retains French source translation."),
    "o07": ("35443 25443 55544", "55543 55544 55343", "Default #1 only drafts test changes and omits timeout constraint; #2 permits new timeouts explicitly. Adaptive #3 rechecks final confirmation without explicit test execution after correction."),
    "o08": ("45543 00000 55543", "55554 55554 55555", "Default invents plot constraints/questions or overbuilds; #2 failed. Adaptive remains proportionate."),
    "o09": ("55544 55544 55543", "55544 55544 55544", "Actual images retained, but browser-navigation/motion boilerplate persists. All generator results remain review-only."),
    "h01": ("35443 35443 35343", "45544 45443 55544", "Default invents inability to access all external links/files and adds responses; actual summary remains. Adaptive #1 weak claim-comparison coverage, #2 final output follows verification."),
    "h02": ("55444 55444 55443", "55334 55434 55555", "Default vague copied Done; adaptive #1 sends before verification and correction would resend; #2 does not verify delivery. No explicit no-send condition in this request."),
    "h03": ("55544 45444 45444", "55444 55554 55444", "Default #2/#3 omit no-new-endpoint. Trigger-specific tests improve. Some write tests without an explicit execution step."),
    "h04": ("00000 00000 55443", "45443 45444 35423", "Default two failures. Adaptive #1 never gives solutions even after attempt, #3 invents beginner/no-transcript and session-limit completion."),
    "h05": ("55555 55554 55543", "55555 55554 55555", "Source sentence and exact entities preserved; default #3 leaks instructional boilerplate."),
    "h06": ("55544 55444 55343", "55444 55544 55444", "120 vs 102 kept unresolved; some final composing steps act as check targets rather than verify completed text."),
    "h07": ("55243 55554 55243", "55243 55233 00000", "Default #2 executes BOTH checks at loop target. Other correction loops recheck only one; adaptive #3 failed."),
    "h08": ("55544 55544 55555", "55544 55544 15423", "Adaptive #3 produces/validates an image prompt instead of creating image. Other outputs request actual image."),
    "b1_h09": ("00000 55443 55443", "00000 45343 00000", "Three failures; usable default copies missing-input loops instead of final verification; adaptive #2 drops no-claim-access exclusion."),
    "b1_h10": ("55322 55443 55544", "55544 55333 55444", "Default #1 leaves literal V and instructional prose; redundant finalization follows checking in several samples."),
    "b2_h11": ("55544 55544 55443", "55544 45443 55544", "Status/action constraints preserved except adaptive #2 replaces undecided launch with no mention of date; #3 asks unnecessary format question."),
    "b2_h12": ("25423 35443 45544", "45444 25423 25423", "Default #1 loses quoted text; #2 forbids quote even as content. Adaptive #2/#3 incorrectly summarize a refusal, reversing customer's actual command content."),
    "b3_h13": ("45443 55443 45544", "55544 35433 55343", "Default unnecessary details and ambiguous confirming-venue goal. Adaptive #2 invents a bug test/simulated organizer response; #3 checks finalization rather than draft."),
    "b3_h14": ("55544 55443 55544", "55444 55444 55554", "Non-default/reload test and key/default constraints remain; some no explicit test execution or unnecessary questions."),
    "b4_h15": ("55444 55444 55544", "25332 55544 55443", "Adaptive #1 deliberately removes source attribution and appends work after Done. Others preserve quotes and evidence-before-choice."),
    "b4_h16": ("55444 15433 55544", "55554 55554 55554", "Default #2 invents Copilot Chat extension as target project, a reviewer-approved critical scope error. Adaptive preserves trigger/binding/dependency constraints."),
    "b5_h17": ("45443 55443 45443", "55554 55554 45544", "Default #1 requests invented timeline; #2 leaks complexity/questions; #3 adds replying with a decision. Adaptive generally faithful."),
    "b5_h18": ("45343 00000 55544", "45544 55554 55554", "Default #2 failed; #1 loop fixes question only. Adaptive #1 calls source statements verified despite absent underlying source."),
    "b6_h19": ("55555 55555 55555", "55554 55555 55554", "One-sentence/no-code/no-analogy faithfully retained."),
    "b6_h20": ("55554 45544 45544", "55554 45544 35432", "Some omit undecided public date; adaptive #3 applies bug-test instructions to a status note."),
    "b7_h21": ("55544 55554 55543", "55554 55544 55554", "Date distinction, two undecided locations and no recommendation remain, with some redundant extraction/questions."),
    "b7_h22": ("25333 55343 25433", "55555 55555 55554", "Default #1/#3 drop updated-invite condition and invent definite unsent invite/refusing Friday; #1 approved. Adaptive preserves conditional availability."),
    "c01": ("04322 24324 14322", "24323 24323 04322", "Default #1 hallucinates prior pricing example; other samples reduce resumed iteration to one draft/template, and adaptive #3 asserts no previous work and adds placeholders."),
    "c02": ("35322 25323 35333", "14323 14323 14323", "Default preserves actual support-assistant workflow but #1 omits independent dimensions and allows done on failure, #2 reduces to six total outputs, #3 loses dimensions/counts. Adaptive discards prior criteria and only edits prompt draft."),
    "c03": ("52443 54443 00000", "52233 55443 52443", "Explicit parallel analysis is serialized by false after-1 edges in default #1/adaptive #1/#3. Adaptive #1 loop calls unresolved disagreements failure. Default #2 has separate roots but no explicit parallel annotation; #3 fails."),
    "c04": ("55543 55554 55554", "55554 55544 55554", "Empty-selection trigger and both final/repeated checks preserved; default #1 needlessly fragments into six steps."),
}

ROUND_TWO = {
    "o01": ("54333 54333 00000", "55554 55554 55554", "Default adds sunset/scattering-angle requirements and checks only an earlier explanation; #3 fails. Adaptive remains concise."),
    "o02": ("00000 55554 52543", "25424 35424 35424", "Default #3 fixes in parallel with diagnosis, approved despite false independence. Adaptive continues per-click fix/completion contradictions; #3 approved but surface-blocked."),
    "o03": ("55543 45343 53343", "00000 45343 55543", "Default #1 overbuilt; #2 unrequested recommendation/early check, #3 analyzes before retrieving prices. Adaptive #1 fails and #2 adds table/early verification."),
    "o04": ("55555 55555 55555", "55554 55555 55554", "Polite request/no apology/no invented deadline retained; only light recipient/context/clarification boilerplate."),
    "o05": ("55554 52343 52343", "00000 00000 52343", "Default #1 now has independent parallel roots; others synthesize/compare in parallel with evidence they need. Two adaptive failures."),
    "o06": ("55555 55555 55555", "55554 00000 00000", "Default now consistently honors existing general correction normalizer; adaptive two failures, one faithful translation."),
    "o07": ("55554 45543 55544", "53543 00000 43433", "Default #2 adds unrequested commit condition. Adaptive #1 fixes before reproduction, #3 analyzes test output in parallel with producing it; #2 fails."),
    "o08": ("55555 55555 55554", "55554 55554 55554", "Faithful gentle bedtime stories, no imposed count."),
    "o09": ("55544 55544 15323", "00000 55544 55544", "Default #3 returns an image prompt rather than image, with capability/limit alternative to completion. Adaptive #1 fails; usable outputs request actual image."),
    "h01": ("25323 15322 45333", "00000 35333 00000", "Default #2 approves questions as done; #1 lacks actual-work/check steps, #3 checks early source receipt. Adaptive only usable sample invents three-bullet output and does not compare claims against each other."),
    "h02": ("55555 55555 55555", "55555 55555 55555", "Faithful Morgan email request and all exclusions."),
    "h03": ("35444 45544 55544", "55444 55544 55554", "Default #1 prescribes preventDefault/stopPropagation as ungrounded solution; #2 only mentions endpoint exclusion at Done. Adaptive preserves trigger and no endpoint."),
    "h04": ("00000 00000 00000", "35433 45433 00000", "Default all fail; adaptive imposes correction bound on learner solving rather than current-turn quality and #1 withholds answer until solved, not merely tried."),
    "h05": ("55555 55555 55554", "55554 55554 55555", "French translation and exact entities preserved."),
    "h06": ("53343 53443 55544", "00000 00000 55343", "Default false parallel/source prerequisites before final comparison. Adaptive two failures; third malformed loop treats correct citation as failure."),
    "h07": ("55554 00000 55554", "55554 00000 00000", "Usable candidates correctly rerun both checks at the correction target; three failures remain."),
    "h08": ("55544 55554 55555", "55544 45544 55444", "Actual image remains in every candidate; adaptive #2 invents Discord integration."),
    "b1_h09": ("45333 00000 45444", "00000 00000 00000", "Four failures; default #1 allows done on missing-data failure; #3 drops no-claim-access exclusion."),
    "b1_h10": ("55543 00000 00000", "55555 55555 55444", "Default faithful planned date but leaked complexity-rule validation; two failures. Adaptive all faithful, third finalizes after check."),
    "b2_h11": ("55555 00000 55444", "55555 00000 55444", "Faithful statuses/owner request; both #2 fail; both #3 add finalization after check."),
    "b2_h12": ("45544 35443 55544", "35443 55344 45443", "Default #1 omits half quoted command, #2 mixes summary/refusal. Adaptive #1/#3 prohibit mentioning quoted command rather than only obeying; #2 lacks actual verifier/correction target."),
    "b3_h13": ("55444 55554 45444", "00000 00000 53343", "Default overbuilt note, #3 goal initially says confirming venue. Adaptive two failures; #3 tone check has no draft prerequisite and finalizes after check."),
    "b3_h14": ("00000 00000 55444", "55344 00000 55544", "Three failures. Default does not explicitly run written test; adaptive #1 loop rechecks writing rather than run step, #3 omits key/default exclusion in operative steps but retains request."),
    "b4_h15": ("55343 45343 00000", "55344 00000 53444", "Two failures. Default adds annual calculations and #2 uses 12 seats ambiguously; adaptive #1 checks analysis not final advice, #3 recommends evidence in parallel with identifying it."),
    "b4_h16": ("00000 00000 55544", "55544 45544 55544", "Default two failures; usable key-repeat fixes preserve binding/dependency exclusions. Adaptive #2 prescribes debounce without evidence."),
    "b5_h17": ("53444 25443 55444", "55554 45554 00000", "Default #1 finalizes before checks; #2 invents charge on destination ChatGPT, approved critical factual expansion; #3 checks tone without draft prerequisite. Adaptive #2 changes investigation/reply timing and weakens reply requirement; #3 fails."),
    "b5_h18": ("55344 55344 55344", "55544 55344 55344", "Attributed unresolved claims preserved. Several loops recheck synthesis/question rather than every affected source/status requirement; adaptive #1 has full verification."),
    "b6_h19": ("55555 55555 55555", "55555 55555 55454", "Faithful one-sentence/code-free/analogy-free semaphore explanations. Adaptive #3 constraint-only verification omits explicit factual check."),
    "b6_h20": ("45554 45554 45544", "55554 45554 53343", "Default omits undecided date instead of keeping uncertainty; adaptive #2 suppresses all date mention and #3 overbuilds/omits draft prerequisite for fact-check and rechecks only one of multiple checks."),
    "b7_h21": ("55554 55444 55344", "55544 00000 00000", "Faithful date/location constraints in usable candidates; default #2 overbuilt and #3 no distinct final verifier; adaptive two failures."),
    "b7_h22": ("55554 55554 55554", "55554 55554 55554", "Conditional Thursday availability and invite/no-Friday exclusions now retained by both policies."),
    "c01": ("05422 05422 05422", "25423 25323 25423", "Default ALL hallucinate last help-center announcement example as previous work. Adaptive ALL reduce continuation to one prompt's formatting/editing rather than resumed work and criteria; rejected."),
    "c02": ("00000 00000 55544", "15323 15322 15423", "Default two failures; third preserves real supplied quality workflow, independent grading and criteria. Adaptive ALL ignore actual supplied workflow in favor of formatting one prompt/draft; rejected or unavailable."),
    "c03": ("00000 00000 00000", "00000 45344 45344", "Four failures. Two adaptive texts preserve parallel roots but correction loops try resolving conflicts rather than preserving disagreement, without actual final verifier."),
    "c04": ("00000 55554 00000", "35443 35443 00000", "Default two failures, one full regression+lint recheck. Adaptive #1/#2 assume a pre-existing test and omit requested test creation; #2 approved despite missing requested artifact action; #3 fails."),
}

ROUND_THREE = {
    "o01": ("55343 55344 55343", "55554 55554 55554", "Default explanations synthesize instead of performing a separate final check; #1/#3 add perception detail and steps. Adaptive concise verified explanations with a small unrequested example."),
    "o02": ("25424 35324 25424", "35424 25434 25424", "All six still fix or complete a per-click guarantee despite double-click tests. Default #1/#3 and adaptive #2 are approved, so scoped event guidance did not solve the critical failure."),
    "o03": ("55444 45441 00000", "55344 45342 45343", "Default #1 concise; #2 ten steps, unrequested best-option choice and questions; #3 fails. Adaptive loops target early source gathering, add recommendations and #2 has twelve-step boilerplate."),
    "o04": ("55555 55555 55555", "55555 55555 55554", "Faithful concise polite drafts, no apology/deadline; adaptive #3 adds missing-detail reporting."),
    "o05": ("00000 52443 00000", "53444 45444 00000", "Default #2 false prerequisite between independent analyses and parallel synthesis; two default failures. Adaptive #1 serializes sources; #2 has proper independent roots but treats disagreement as failure; #3 fails."),
    "o06": ("55555 55455 55455", "45554 15433 55554", "Default keeps the final French request. Adaptive #1 weakens the explicit no-summary exclusion; #2 restores the retracted email in its preamble (rejected); #3 faithful."),
    "o07": ("55554 55544 55544", "00000 55544 00000", "Default fixes and reruns the full suite; #3 awkward Done grammar. Adaptive has two failures and one workflow with appended questions after Done."),
    "o08": ("55555 55343 55344", "55343 00000 00000", "Default #1 concise; #2/#3 overprescribe story structure without a distinct final check. Adaptive #1 checks only the introduction before the rest; two failures."),
    "o09": ("15423 55554 55544", "55554 55554 55555", "Default #1 returns a single-line image prompt rather than creating the image; the other five create and inspect the requested image."),
    "h01": ("55344 55454 45423", "45423 55344 35333", "Default #1 lacks a final verifier; #3 and adaptive #1 permit Done on prompting for or failing to get the source. Adaptive #3 invents three bullets and replaces comparing claims with checking them."),
    "h02": ("55555 55554 55555", "55334 55555 55555", "Faithful Morgan requests; adaptive #1 sends before verification with no delivery check (rejected; ambiguous send authorization, not classified critical)."),
    "h03": ("25434 45544 55444", "55444 45534 45544", "Default #1 approved per-click guarantee contradicts double-click requirement; #2 drops no-new-endpoint; #3 has no explicit test run. Adaptive keeps double-click semantics but #2 has a nonsensical limit clause."),
    "h04": ("00000 00000 00000", "55554 55554 33211", "All default samples fail. Adaptive #1/#2 tutor one turn at a time without a fixed total; #3 invents assessment, summaries, parallel monitoring and three Done clauses."),
    "h05": ("55555 55555 55554", "55555 55555 55555", "Exact French translation with Nora, date and amount unchanged."),
    "h06": ("55444 55554 54443", "55554 55444 55444", "All preserve 120 versus 102 without averaging. Several synthesize without a distinct final verifier; default #3 serializes source analyses and is overbuilt."),
    "h07": ("55554 55552 55554", "55554 55554 55554", "All usable outputs rerun both tests and lint after the final edit and loop correction. Default #2 adds three unnecessary questions."),
    "h08": ("55555 35423 55555", "55555 55555 55555", "Image created and inspected for no lettering/watermark. Default #2 prescribes a specific site and has a garbled Done clause."),
    "b1_h09": ("00000 00000 00000", "00000 00000 00000", "All six samples fail without usable output."),
    "b1_h10": ("55555 55555 55554", "55554 55554 55555", "Planned May 8 and post-testing confirmation retained without guarantees or passed-testing claims."),
    "b2_h11": ("55555 55555 55554", "00000 55444 55555", "Prototype, pending review and undecided launch retained with owner request; adaptive #1 fails and #2 edits after its check."),
    "b2_h12": ("45444 45344 25433", "35443 45344 25343", "Usable outputs avoid following the embedded command, but most ban mentioning the quoted command in a summary; default #3 and adaptive #3 mischaracterize the message as a request to summarize a missing report."),
    "b3_h13": ("55555 55555 55554", "55554 55554 55554", "June 18 venue-confirmation note without booking or arrival time; some vague Done criteria."),
    "b3_h14": ("55544 45554 55544", "55444 55444 55444", "Non-default regression and reload retained. Default #2 omits key/default exclusion; default asks optional questions; adaptive writes the test without an explicit run step."),
    "b4_h15": ("54441 00000 55344", "45443 45344 00000", "Default #1 twelve-step table/boilerplate; #2 and adaptive #3 fail. Adaptive #1 disclaims recommendation wording; #2 focuses validation on the lower quote and loops before its verifier."),
    "b4_h16": ("45333 55554 55543", "55554 00000 55554", "Default #1 never runs the regression and invents documentation; #3 embeds questions. Others preserve binding/no dependency and rerun final checks; adaptive #2 fails."),
    "b5_h17": ("55554 55554 55555", "55555 55544 55555", "Draft-for-approval investigation response without refund promise; some omit explicit no-send wording."),
    "b5_h18": ("55444 55555 55555", "55444 55555 55444", "Attribution and unresolved status retained; several outputs synthesize without a distinct final verifier."),
    "b6_h19": ("55555 55555 55555", "55554 55554 55554", "One sentence, no code, no analogy; adaptive checks omit accuracy."),
    "b6_h20": ("55555 55555 55555", "55555 55554 55554", "Internal beta status and bug-report request retained without public launch or date promise."),
    "b7_h21": ("55555 55444 55555", "55554 55554 55544", "Confirmed dates/estimates/two undecided locations retained without recommendation; default #2 lacks a separate verifier."),
    "b7_h22": ("55555 45554 45554", "55554 35444 55554", "Conditional Thursday availability usually retained. Default #2/#3 turn not agreeing into refusing Friday; adaptive #2 drops the updated-invite condition (rejected)."),
    "c01": ("14332 34434 35434", "24333 24323 24333", "No sample preserves the unresolved reference and waits for essential context. Default #1 invents a ChatGPT target prompt; others assume a current prompt draft, history or shared contract; adaptive #2 allows Done at the limit."),
    "c02": ("00000 54453 54434", "14323 14334 14222", "Default #2/#3 retain the supplied workflow and criteria with imperfect check/loop structure; #1 fails. All adaptive samples ignore the supplied workflow and edit one prompt draft."),
    "c03": ("00000 00000 00000", "00000 00000 00000", "All six parallel-evidence samples fail without usable output despite the new two-source teaching."),
    "c04": ("55554 55554 55555", "35444 35443 55555", "Default samples add the empty-selection test and rerun test plus lint after corrections. Adaptive #1/#2 are approved but never create the requested regression test; #3 faithful."),
}

def publish(raw_path, assignments, output_path, critical=()):
    raw = json.loads(Path(raw_path).read_text())
    rows = []
    groups = defaultdict(list)
    for sample in raw["samples"]:
        if sample["mode"] != "prompt":
            continue
        legacy, adaptive, evidence = assignments[sample["case_id"]]
        vector = (legacy if sample["policy"] == "legacy" else adaptive).split()[sample["sample"] - 1]
        scores = [int(value) for value in vector]
        assert len(scores) == 5 and all(0 <= value <= 5 for value in scores)
        if not sample["usable"]:
            assert scores == [0] * 5
        key = (sample["case_id"], sample["policy"], sample["sample"])
        grade = sum(scores) * 0.4
        row = {
            "case_id": sample["case_id"], "policy": sample["policy"], "sample": sample["sample"],
            "suite": sample["suite"], "dimensions_out_of_5": dict(zip(DIMENSIONS, scores)),
            "grade_out_of_10": round(grade, 2), "evidence": evidence,
            "critical_fidelity": key in critical, "reviewer_approved": sample.get("quality") is not None
                and sample["quality"]["status"] in ("checked", "corrected"),
            "auto_paste_eligible": sample["auto_paste_eligible"],
        }
        rows.append(row)
        groups[(sample["suite"], sample["policy"])].append(row)
    assert len(rows) == len(assignments) * 6, "every case needs three samples for both policies"
    result = {
        "raw_report": raw_path, "grader": "Copilot reading final outputs independently of local reviewer verdicts",
        "rubric": "Five fixed equally weighted dimensions /5; total /25 multiplied by 10. Failures/incomplete output = all zero. Blocked complete text is graded, not excluded.",
        "limitations": "Independent of the runtime model reviewer, not blinded human grading; finite synthetic sample, fixed request-ID seeds are not paired across altered call counts.",
        "samples": rows,
        "means_out_of_10": {":".join(k): round(sum(r["grade_out_of_10"] for r in rs) / len(rs), 3) for k, rs in groups.items()},
        "approved_below_7": [r for r in rows if r["reviewer_approved"] and r["grade_out_of_10"] < 7],
        "approved_critical_fidelity": [r for r in rows if r["reviewer_approved"] and r["critical_fidelity"]],
    }
    prompt_samples = [sample for sample in raw["samples"] if sample["mode"] == "prompt"]
    result["operations"] = {
        "sample_count": len(prompt_samples),
        "usable": sum(sample["usable"] for sample in prompt_samples),
        "auto_paste_eligible": sum(sample["auto_paste_eligible"] for sample in prompt_samples),
        "structure_repairs": sum(sample["structure_repair_attempts"] for sample in prompt_samples),
        "reviews": sum((sample.get("quality") or {}).get("review_calls", 0) for sample in prompt_samples),
        "rewrites": sum((sample.get("quality") or {}).get("rewrite_calls", 0) for sample in prompt_samples),
        "quality_statuses": dict(Counter((sample.get("quality") or {}).get("status", "failed") for sample in prompt_samples)),
        "blocks": dict(Counter(sample["block_reason"] for sample in prompt_samples if sample["block_reason"])),
        "deadline_exhausted": sum((sample.get("quality") or {}).get("deadline_exhausted", False) for sample in prompt_samples),
        "summed_wall_minutes": round(sum(sample["wall_elapsed_ms"] for sample in prompt_samples) / 60000, 2),
        "warning_observability": "Routing warning strings are not serialized by this harness; quality statuses and surface/quality blocks are retained, not relabeled as warning counts.",
    }
    result["wall_seconds"] = {}
    for group in groups:
        times = sorted(sample["wall_elapsed_ms"] / 1000 for sample in prompt_samples
                       if (sample["suite"], sample["policy"]) == group)
        result["wall_seconds"][":".join(group)] = {
            "mean": round(sum(times) / len(times), 3),
            "p95_nearest_rank": round(times[ceil(.95 * len(times)) - 1], 3),
            "maximum": round(max(times), 3),
        }
    result["individual_batch_gates"] = {
        "all_four_means_at_least_8_5": all(mean >= 8.5 for mean in result["means_out_of_10"].values()),
        "no_approved_below_7": not result["approved_below_7"],
        "no_approved_critical_fidelity": not result["approved_critical_fidelity"],
        "at_least_95_percent_usable": result["operations"]["usable"] / len(prompt_samples) >= .95,
        "at_least_three_samples_each": all(
            len([sample for sample in prompt_samples if sample["case_id"] == case_id and sample["policy"] == policy]) >= 3
            for case_id in assignments for policy in ("legacy", "adaptive")),
    }
    result["passes_individual_batch"] = all(result["individual_batch_gates"].values())
    result["consistency_requirement"] = "Two consecutive full passing batches are required separately; one passing batch is insufficient."
    Path(output_path).write_text(json.dumps(result, indent=2) + "\n")
    return result

if __name__ == "__main__":
    result = publish("eval/quality-review-batch-7.json", BASELINE,
                     "eval/quality-review-batch-7-prompt-grades.json",
                     {("o02", "legacy", 2), ("o02", "adaptive", 2),
                      ("o06", "legacy", 1), ("o06", "legacy", 2)})
    print(json.dumps({k: v if not isinstance(v, list) else len(v)
                      for k, v in result.items() if k.startswith(("means", "approved"))}, indent=2))
    if Path("eval/quality-review-prompt-round-1.json").exists():
        result = publish("eval/quality-review-prompt-round-1.json", ROUND_ONE,
                         "eval/quality-review-prompt-round-1-grades.json",
                         {("o02", "adaptive", 1), ("o02", "adaptive", 2), ("o02", "adaptive", 3),
                          ("o06", "legacy", 1), ("o06", "legacy", 2), ("o06", "legacy", 3),
                          ("o07", "legacy", 2), ("h08", "adaptive", 3),
                          ("b4_h16", "legacy", 2), ("b7_h22", "legacy", 1), ("b7_h22", "legacy", 3),
                          ("c01", "legacy", 1), ("c01", "adaptive", 3),
                          ("c02", "adaptive", 1), ("c02", "adaptive", 2), ("c02", "adaptive", 3)})
        print(json.dumps({k: v if not isinstance(v, list) else len(v)
                          for k, v in result.items() if k.startswith(("means", "approved"))}, indent=2))
    if Path("eval/quality-review-prompt-round-2.json").exists():
        result = publish("eval/quality-review-prompt-round-2.json", ROUND_TWO,
                         "eval/quality-review-prompt-round-2-grades.json",
                         {("o02", "adaptive", 1), ("o02", "adaptive", 2), ("o02", "adaptive", 3),
                          ("o09", "legacy", 3), ("h01", "legacy", 2), ("b5_h17", "legacy", 2),
                          ("c01", "legacy", 1), ("c01", "legacy", 2), ("c01", "legacy", 3),
                          ("c02", "adaptive", 1), ("c02", "adaptive", 2), ("c02", "adaptive", 3),
                          ("c04", "adaptive", 1), ("c04", "adaptive", 2)})
        print(json.dumps({k: v if not isinstance(v, list) else len(v)
                          for k, v in result.items() if k.startswith(("means", "approved"))}, indent=2))
    if Path("eval/quality-review-prompt-round-3.json").exists():
        result = publish("eval/quality-review-prompt-round-3.json", ROUND_THREE,
                         "eval/quality-review-prompt-round-3-grades.json",
                         {("o02", "legacy", 1), ("o02", "legacy", 2), ("o02", "legacy", 3),
                          ("o02", "adaptive", 1), ("o02", "adaptive", 2), ("o02", "adaptive", 3),
                          ("o06", "adaptive", 2), ("o09", "legacy", 1), ("h03", "legacy", 1),
                          ("b2_h12", "legacy", 3), ("b2_h12", "adaptive", 3), ("b7_h22", "adaptive", 2),
                          ("c01", "legacy", 1),
                          ("c02", "adaptive", 1), ("c02", "adaptive", 2), ("c02", "adaptive", 3),
                          ("c04", "adaptive", 1), ("c04", "adaptive", 2)})
        print(json.dumps({k: v if not isinstance(v, list) else len(v)
                          for k, v in result.items() if k.startswith(("means", "approved"))}, indent=2))
