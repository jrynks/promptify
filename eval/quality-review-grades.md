# Independent semantic quality grades

This report scores the final text from the synthetic local-model runs independently of the runtime
reviewer's verdict. The reviewer status is used only after scoring to test the accepted-output floor.
No user history, screen text, microphone input or native paste was used. Failed jobs and incomplete
truncations receive 0; complete review/copy-only text is graded normally.

## Rubric

Scores are 0–10, assigned to each individual sample. 0 means no complete usable text or a materially
misleading result; 1–3 means the request is largely unmet or materially distorted; 4–6 means
substantial omissions, unsupported additions or contradictions; 7 means usable with a notable
weakness; 8 means good with minor issues; 9 means strong and faithful; 10 means fully faithful,
complete and proportionate. Prompt scores consider intent, exclusions/details, workflow coherence,
verification/completion and proportionality. Dictation scores consider meaning, exact facts and
commitments, paragraph/edit-command fidelity, selected tone and unsupported additions. The per-sample
vectors below are ordered by sample index; group means include every score, including failed samples.

## Round 1: pre-refinement baseline

Run: selected installed `qwen3.5-9b-q4km`, one warmed worker, 120-second CLI job deadline,
temperature 0.3, top-k 40, top-p 0.9, min-p 0.05, worker-request-ID seed, 768 generation tokens,
two structure repairs, 6,000-character output cap. 124 synthetic samples: 9 original requests × 2
policies × 3 samples; 8 held-out requests × 2 policies; 3 dictation inputs × 6 tones × 3 samples.
The default model selection was read without repairing or saving settings. `NoHistory`, synthetic
`FixedContext`, and `PrintInserter` were used throughout.

### Prompt requests

| Case | Policy | Sample grades /10 | Mean | Independent evidence |
| --- | --- | --- | ---: | --- |
| o01 explanation | adaptive | 9, 0, 9 | 6.00 | Two concise, accurate Rayleigh-scattering workflows; one failed with no final text. |
| o01 explanation | legacy | 0, 9, 8 | 5.67 | One output copied the graph contract and truncated; the other two were usable. |
| o02 double-click fix | adaptive | 6, 8.5, 0 | 4.83 | First says one request “per click,” which can preserve the double-click defect; second is sound; third failed. |
| o02 double-click fix | legacy | 8.5, 8.5, 0 | 5.67 | First two preserve the fix/test/API constraints; third copied the contract and truncated. |
| o03 low-cost research | adaptive | 4, 9, 7 | 6.67 | One wrongly assumes browsing is unavailable; one requests current sources; one adds risks/tests/recommendation not requested. |
| o03 low-cost research | legacy | 8, 9, 7.5 | 8.17 | Mostly complete and source-aware; the last adds risk-testing and recommendation requirements. |
| o04 meeting-notes email | adaptive | 7, 0, 7 | 4.67 | Both drafts preserve no-apology/no-deadline, but ask unnecessary questions or allow placeholders; one failed. |
| o04 meeting-notes email | legacy | 0, 0, 0 | 0.00 | Failed or returned an incomplete copy of the graph contract with invented placeholders. |
| o05 independent evidence | adaptive | 9, 7.5, 8.5 | 8.33 | Independent analyses and disagreement preservation are strong; one sample has a weak merge/check flow. |
| o05 independent evidence | legacy | 8.5, 8.5, 8.5 | 8.50 | Preserves independent sources and citations; adds some unnecessary questions/report scaffolding. |
| o06 corrected French translation | adaptive | 9, 9, 9 | 9.00 | Correctly follows the final translation request and identifies that source text must be supplied. |
| o06 corrected French translation | legacy | 0, 0, 0 | 0.00 | Failed or truncated while copying the graph contract. |
| o07 flaky API tests | adaptive | 8, 8, 8 | 8.00 | Root-cause, no-skip/no-timeout constraints and post-fix verification are retained. |
| o07 flaky API tests | legacy | 8, 0, 0 | 2.67 | First sample is usable; two have no final text. |
| o08 bedtime story | adaptive | 0, 9, 9 | 6.00 | Two preserve gentle/no-count constraints; one failed. |
| o08 bedtime story | legacy | 9, 0, 7.5 | 5.50 | One good story prompt, one failed; one adds age/setting assumptions and is overbuilt. |
| o09 image creation | adaptive | 8, 8, 0 | 5.33 | Two relevant image-generation workflows; one failed. Tool/API wording is not always grounded. |
| o09 image creation | legacy | 0, 8, 8 | 5.33 | Two useful, review-only generator prompts; one failed. |

### Held-out requests

| Case | Policy | Sample grade /10 | Independent evidence |
| --- | --- | ---: | --- |
| h01 missing reference | adaptive | 7.5 | Requests the missing source, but declares dependent analyses parallel. |
| h01 missing reference | legacy | 9 | Explicitly refuses to invent or claim access and asks for the source. |
| h02 negation | adaptive | 0 | No final text. |
| h02 negation | legacy | 0 | No final text. |
| h03 semantic double-click | adaptive | 7 | Preserves exactly-one-request and no-endpoint intent, but prescribes an unverified debounce/lock approach. |
| h03 semantic double-click | legacy | 8.5 | Preserves the behavior, regression test, API and test execution. |
| h04 interactive tutoring | adaptive | 9 | One question at a time, waits, adapts, and does not invent an interaction count. |
| h04 interactive tutoring | legacy | 5 | Invents a fixed five-question session despite the no-fixed-total requirement. |
| h05 translation | adaptive | 8.5 | Preserves the requested names/date/amount and translation scope. |
| h05 translation | legacy | 8.5 | Gives a natural French date rendering and preserves the value, with minor ambiguity about the “unchanged” formatting requirement. |
| h06 evidence disagreement | adaptive | 8.5 | Preserves 120 vs. 102 and separates claims from verification. |
| h06 evidence disagreement | legacy | 6.5 | Preserves both numbers but invents a limitation about external verification access. |
| h07 post-verification edit | adaptive | 8.5 | Explicitly reruns tests and lint after the final edit. |
| h07 post-verification edit | legacy | 0 | Incomplete graph-contract output. |
| h08 media creation | adaptive | 9 | Directly requests the image and checks both visual constraints. |
| h08 media creation | legacy | 9 | Direct image request with review and limitation reporting. |

### Dictation: three varied inputs per tone, three samples each

| Input | Tone | Sample grades /10 | Mean | Independent evidence |
| --- | --- | --- | ---: | --- |
| d01 quoted instruction | clean_transcript | 10, 10, 10 | 10.00 | Deterministic cleanup preserves both paragraphs, the exact quote and the refusal to follow it. |
| d01 quoted instruction | natural | 5, 9.5, 9.5 | 8.00 | One sample drops the quoted paragraph; two preserve it exactly. |
| d01 quoted instruction | casual | 9, 9, 9 | 9.00 | Preserves the amount, exclusion, quotation and paragraph boundary; source was already conversational. |
| d01 quoted instruction | formal | 5, 9, 9 | 7.67 | One sample omits the quote/paragraph; two retain them. |
| d01 quoted instruction | concise | 9, 9, 9 | 9.00 | Preserves the quoted material, exclusion and paragraph boundary. |
| d01 quoted instruction | unhinged | 7, 7, 7 | 7.00 | Facts and quote stay intact, but the output is not noticeably playful or irreverent. |
| d02 numbers and exclusion | clean_transcript | 10, 10, 10 | 10.00 | Exact $249/$294, three copies, March 14, exclusion and paragraph break. |
| d02 numbers and exclusion | natural | 9.5, 9.5, 9.5 | 9.50 | Preserves all details and paragraph separation. |
| d02 numbers and exclusion | casual | 9.5, 9.5, 9.5 | 9.50 | Preserves all details and paragraph separation. |
| d02 numbers and exclusion | formal | 9, 9, 9 | 9.00 | Adds “Please” but preserves the exclusion and facts. |
| d02 numbers and exclusion | concise | 7, 7, 7 | 7.00 | Keeps the content but merges the explicitly requested second paragraph. |
| d02 numbers and exclusion | unhinged | 7, 7, 8 | 7.33 | Facts remain intact; two outputs barely alter the source tone. |
| d03 scratch-that commitment | clean_transcript | 10, 10, 10 | 10.00 | Correctly removes Lee and retains Sam, Wednesday 3 p.m., $1,250 and paragraph boundaries. |
| d03 scratch-that commitment | natural | 9.5, 9.5, 9.5 | 9.50 | Corrected commitment, amount and paragraph structure preserved. |
| d03 scratch-that commitment | casual | 9.5, 9.5, 9.5 | 9.50 | Same, with natural conversational phrasing. |
| d03 scratch-that commitment | formal | 9, 9, 9 | 9.00 | Corrected commitment and exact amount preserved. |
| d03 scratch-that commitment | concise | 2, 9, 2 | 4.33 | Samples 1 and 3 restore the deleted Lee commitment; sample 2 is correct. |
| d03 scratch-that commitment | unhinged | 2, 7, 7 | 5.33 | Sample 1 restores Lee; the other two preserve the final commitment but miss the requested style. |

### Round 1 gate results

| Gate | Result |
| --- | --- |
| Original prompt mean, legacy ≥8.5 | **Fail: 4.61/10** |
| Original prompt mean, adaptive ≥8.5 | **Fail: 6.54/10** |
| Held-out prompt mean, legacy ≥8.5 | **Fail: 5.81/10** |
| Held-out prompt mean, adaptive ≥8.5 | **Fail: 7.25/10** |
| Dictation means ≥8.5 by tone | **Fail:** Natural 9.00, Casual 9.33, Formal 8.56, Concise 6.78, Unhinged 6.56; Clean parity 10.00 |
| No approved output below 7 | **Fail:** an approved/inserted tutoring prompt invented a five-question total (5/10). |
| Zero critical fidelity changes in approved outputs | **Fail:** the same approved tutoring prompt imposes a fixed five-question session. |
| ≥95% complete usable text | **Fail:** 104/124 (83.9%). Corrected counting excludes 13 no-text failures and 7 truncated outputs. |
| Deadline exhaustion | 0/124 |
| Auto-paste eligible | 40/124 (32.3%): original 4/54, held-out 2/16, rewrite dictation 25/45, Clean 9/9. |

Prompt wall-time means (p95): original legacy 32.3 s (56.1 s), original adaptive 22.2 s (34.3 s),
held-out legacy 34.2 s (62.4 s), held-out adaptive 29.0 s (40.5 s). Rewrite dictation means ranged
from 3.7–6.0 s by tone; Clean averaged 14 ms. All 20 unusable results are retained in the raw JSON
with their outcome, status, and output/truncation data. Among complete but non-inserted text, 63
results were quality-review-only and one was blocked by generator-surface capability.

Critical fidelity errors in rejected/copy-only text were also observed: two Concise and one Unhinged
scratch-that rewrites restored the deleted Lee commitment; one Natural and one Formal sample omitted
the quoted instruction paragraph. Those outputs were not inserted. This is evidence that review-only
copy recovery is not itself a quality guarantee.

The runtime review verdicts and output text are retained in
[quality-review-results.json](./quality-review-results.json). The grades above use the final text,
not the reviewer's verdict or a self-assigned score. Scores are finite-corpus evidence, not a claim
that every request is safe or correct.

## Round 2: after prompt and transcript-fidelity refinements

Run: same installed `qwen3.5-9b-q4km`, corpus, warm-worker protocol, sample counts, 120-second CLI
deadline, and sampling settings as round 1. 124 samples; no history, native paste, model switch or
download. The same independent 0–10 rubric was applied to each final result, including complete
copy-only results. Failed outputs and truncations score 0.

### Original prompt requests

Grades are per-sample vectors, ordered by sample number.

| Case | Legacy grades /10 | Mean | Adaptive grades /10 | Mean |
| --- | --- | ---: | --- | ---: |
| o01 explanation | 8, 8, 9 | 8.33 | 8.5, 8.5, 8.5 | 8.50 |
| o02 double-click fix | 6.5, 9, 6.5 | 7.33 | 9, 6.5, 6.5 | 7.33 |
| o03 low-cost research | 7.5, 8.5, 8.5 | 8.17 | 8.5, 8.5, 8 | 8.33 |
| o04 meeting-notes email | 5.5, 0, 0 | 1.83 | 7.5, 7.5, 7.5 | 7.50 |
| o05 independent evidence | 8.5, 8.5, 0 | 5.67 | 9, 9, 9 | 9.00 |
| o06 corrected French translation | 8, 4, 0 | 4.00 | 8.5, 6.5, 7.5 | 7.50 |
| o07 flaky API tests | 6.5, 6.5, 8 | 7.00 | 8.5, 8.5, 8.5 | 8.50 |
| o08 bedtime story | 8.5, 0, 7.5 | 5.33 | 8.5, 9, 7.5 | 8.33 |
| o09 image creation | 8.5, 8.5, 8.5 | 8.50 | 8.5, 7.5, 0 | 5.33 |

Legacy mean: **6.24/10**; adaptive mean: **7.81/10**. The two policies improve over round 1
(4.61 and 6.54 respectively), but neither meets the 8.5 target. Stronger adaptive results retain
the final translation correction, testing constraints, and creative exclusions. Weak results either
fail to produce text, echo the graph contract, or ask unnecessary questions/add unsupported scaffolding.

### Held-out requests

| Case | Legacy grade /10 | Adaptive grade /10 | Independent evidence |
| --- | ---: | ---: | --- |
| h01 missing reference | 0 | 9 | Legacy correction failed graph validation; adaptive correctly requests the absent evidence and does not claim to have read it. |
| h02 negation | 0 | 8.5 | Legacy correction failed graph validation; adaptive preserves the no-apology/no-deadline/no-lateness constraints. |
| h03 semantic double-click | 9 | 9 | Both preserve the one-request requirement and regression test; legacy adds a reasonable implementation suggestion. |
| h04 interactive tutoring | 8 | 8.5 | Both preserve one-question-at-a-time interaction; legacy asks an unnecessary background question and adaptive adds a needless post-answer clarification step. |
| h05 translation | 8 | 8 | Both retain the exact source facts; the repeated “word-for-word” language is awkward for natural translation. |
| h06 evidence disagreement | 9 | 8.5 | Both retain 120 versus 102 without resolving or averaging the disagreement. |
| h07 post-verification edit | 9 | 0 | Legacy requires both checks after the final edit; adaptive correction failed graph validation. |
| h08 media creation | 8.5 | 8.5 | Both directly request image creation and preserve the no-lettering/no-watermark constraints. |

Held-out mean: legacy **6.44/10**; adaptive **7.50/10**. Neither meets 8.5.

### Dictation

Each tone has three samples for each of the three inputs. Vectors below are d01, d02, and d03,
with three samples per input in order.

| Tone | d01 grades | d02 grades | d03 grades | Mean |
| --- | --- | --- | --- | ---: |
| Clean transcript | 10, 10, 10 | 10, 10, 10 | 10, 10, 10 | 10.00 |
| Natural | 10, 10, 4 | 9, 10, 10 | 9.5, 9.5, 9.5 | 9.06 |
| Casual | 4, 9, 9 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 8.78 |
| Formal | 4, 4, 4 | 6.5, 6.5, 6.5 | 6.5, 9.5, 9.5 | 6.33 |
| Concise | 10, 10, 10 | 10, 10, 10 | 9.5, 9.5, 9.5 | 9.83 |
| Unhinged | 7.5, 7.5, 7.5 | 7, 7, 7 | 8.5, 7, 7 | 7.33 |

Formal and Unhinged miss 8.5. Several d01 rewrite samples omit the exact quoted paragraph; all
such outputs were rejected and remained copy-only. The effective-transcript change prevented any
d03 rewrite from restoring the scratched-out Lee commitment; the final Sam commitment and `$1,250`
were preserved. However, all three approved Formal d02 rewrites merged the explicitly requested
paragraphs. This is a critical fidelity failure and each scores below 7. Unhinged output was often
too close to the source to meet the requested tone.

### Round 2 gates and operational measures

| Gate | Result |
| --- | --- |
| Original prompt mean, legacy ≥8.5 | **Fail: 6.24/10** |
| Original prompt mean, adaptive ≥8.5 | **Fail: 7.81/10** |
| Held-out prompt mean, legacy ≥8.5 | **Fail: 6.44/10** |
| Held-out prompt mean, adaptive ≥8.5 | **Fail: 7.50/10** |
| Dictation means ≥8.5 by tone | **Fail:** Natural 9.06, Casual 8.78, Formal 6.33, Concise 9.83, Unhinged 7.33; Clean 10.00 |
| No approved output below 7 | **Fail:** the inserted o07 legacy sample 1/3 imposes three consecutive full-suite passes and no warnings (6.5/10); all three Formal d02 inserted samples merge two spoken paragraphs (6.5/10). |
| Zero critical fidelity changes in approved outputs | **Fail:** o07 legacy sample 1/3 invents three consecutive executions; all three approved Formal d02 samples erase the explicit paragraph break. |
| ≥95% complete usable text | **Fail: 115/124 (92.7%)**; 7 correction rewrites failed graph validation and 2 outputs were truncated. |
| Deadline exhaustion | 0/124 |
| Auto-paste eligible | 48/124 (38.7%). The other complete results were review-only; unusable outputs were never eligible. |

Prompt wall-time means (p95): original legacy 34.3 s (52.5 s), original adaptive 24.3 s (39.8 s),
held-out legacy 28.4 s (28.8 s), and held-out adaptive 28.9 s (38.9 s). Dictation rewrite means
were Natural 3.8 s, Casual 5.1 s, Formal 3.7 s, Concise 3.9 s and Unhinged 5.2 s; Clean averaged
15 ms. No measured job exhausted its deadline. All 9 unusable outputs and all 115 usable outputs,
including copy-only text, are retained in [quality-review-round-2.json](./quality-review-round-2.json).

Of the seven no-text failures, three came from a targeted correction that failed graph validation
after an initially valid draft had passed structural validation; the other four were initial
graph-generation/repair failures. The next refinement preserves the valid draft as explicitly
rejected/copy-only in the first case, without authorizing insertion or exposing the invalid
correction. That recovered three results; the four initial graph failures remained in round 3.

## Round 3: final bounded refinement

Run: same selected installed `qwen3.5-9b-q4km`, same 124-case corpus and sample counts, same
temperature/top-k/top-p/min-p settings, 768 generation tokens, and 120-second CLI deadline. The
only behavior changes since round 2 were preserving a valid draft as copy-only when its targeted
rewrite failed validation, and a clearer requested-style rubric for dictation. No history, native
paste, model switch or download was used.

### Original prompt requests

| Case | Legacy grades /10 | Mean | Adaptive grades /10 | Mean |
| --- | --- | ---: | --- | ---: |
| o01 explanation | 8, 8, 9 | 8.33 | 8.5, 8.5, 8.5 | 8.50 |
| o02 double-click fix | 6.5, 9, 6.5 | 7.33 | 9, 6.5, 6.5 | 7.33 |
| o03 low-cost research | 7.5, 8.5, 8.5 | 8.17 | 8.5, 8.5, 8 | 8.33 |
| o04 meeting-notes email | 5.5, 0, 0 | 1.83 | 7.5, 7.5, 7.5 | 7.50 |
| o05 independent evidence | 8.5, 8.5, 7.5 | 8.17 | 9, 9, 9 | 9.00 |
| o06 corrected French translation | 8, 4, 3.5 | 5.17 | 8.5, 6.5, 7.5 | 7.50 |
| o07 flaky API tests | 6.5, 6.5, 8 | 7.00 | 8.5, 8.5, 8.5 | 8.50 |
| o08 bedtime story | 8.5, 0, 7.5 | 5.33 | 8.5, 9, 7.5 | 8.33 |
| o09 image creation | 8.5, 8.5, 8.5 | 8.50 | 8.5, 7.5, 7.5 | 7.83 |

Original means: legacy **6.65/10**, adaptive **8.09/10**. Adaptive is improved over round 2 but
both policies remain below the 8.5 target.

### Held-out requests

| Case | Legacy grade /10 | Adaptive grade /10 | Independent evidence |
| --- | ---: | ---: | --- |
| h01 missing reference | 8 | 9 | Both now produce text that states the report is missing and asks for its content; adaptive is especially clear not to claim access. |
| h02 negation | 0 | 8.5 | Legacy correction remains structurally invalid; adaptive preserves the exclusions. |
| h03 semantic double-click | 9 | 9 | Both preserve exactly one request on double-click and require a regression test. |
| h04 interactive tutoring | 8 | 8.5 | Both preserve one-question-at-a-time interaction and wait for the learner. |
| h05 translation | 8 | 8 | Both retain the exact supplied sentence and exclusions; “word-for-word” remains unnecessarily restrictive. |
| h06 evidence disagreement | 9 | 8.5 | Both preserve 120 versus 102 and do not decide between sources. |
| h07 post-verification edit | 9 | 8.5 | Both require the tests and lint to be rerun after the final edit. |
| h08 media creation | 8.5 | 8.5 | Both directly request image creation and preserve the visual exclusions. |

Held-out means: legacy **7.44/10**; adaptive **8.56/10**. Adaptive clears this one gate; legacy does not.

### Dictation

Each tone has three samples for each input. Vectors are d01, d02, and d03, three samples per input.

| Tone | d01 grades | d02 grades | d03 grades | Mean |
| --- | --- | --- | --- | ---: |
| Clean transcript | 10, 10, 10 | 10, 10, 10 | 10, 10, 10 | 10.00 |
| Natural | 4, 4, 4 | 9.5, 10, 10 | 9.5, 9.5, 9.5 | 7.78 |
| Casual | 9, 9, 9 | 9.5, 9.5, 9.5 | 10, 9.5, 10 | 9.44 |
| Formal | 9, 9, 4 | 6.5, 6.5, 6.5 | 6.5, 9.5, 9.5 | 7.44 |
| Concise | 4, 9, 9 | 10, 10, 10 | 9.5, 9.5, 9.5 | 8.94 |
| Unhinged | 7.5, 7.5, 7.5 | 7, 7, 7 | 6.5, 7, 7 | 7.11 |

Natural, Formal and Unhinged miss 8.5. The better d01 Casual and Formal rewrites preserve the
quoted paragraph; Natural and Concise still have copy-only outputs that omit it. The d03 scratch-that
change continued to remove Lee and preserve Sam and `$1,250`. However, approved Formal d02 outputs
all collapse the explicitly requested paragraph boundary; Formal d03 sample 1 also merges its
paragraphs. Approved Unhinged d03 sample 1 adds “this Wednesday” and “sharp,” sharpening a date/time
commitment not present in the source. These are independent critical-fidelity failures, not reviewer
scores.

### Round 3 gates and operational measures

| Gate | Result |
| --- | --- |
| Original prompt mean, legacy ≥8.5 | **Fail: 6.65/10** |
| Original prompt mean, adaptive ≥8.5 | **Fail: 8.09/10** |
| Held-out prompt mean, legacy ≥8.5 | **Fail: 7.44/10** |
| Held-out prompt mean, adaptive ≥8.5 | **Pass: 8.56/10** |
| Dictation means ≥8.5 by tone | **Fail:** Natural 7.78, Casual 9.44, Formal 7.44, Concise 8.94, Unhinged 7.11; Clean 10.00 |
| No approved output below 7 | **Fail:** six independently graded inserted outputs score 6.5/10: o07 legacy #1, all three Formal d02 samples, Formal d03 #1, and Unhinged d03 #1. |
| Zero critical fidelity changes in approved outputs | **Fail:** o07 legacy #1 invents three consecutive test runs; four Formal samples merge explicit paragraphs; Unhinged d03 #1 adds “this” and “sharp” to the time commitment. |
| ≥95% complete usable text | **Pass: 120/124 (96.8%)**; four structurally invalid initial prompts remain no-text failures. |
| Deadline exhaustion | 0/124 |
| Auto-paste eligible | 49/124 (39.5%); all rejected/unavailable reviews and invalid-correction fallbacks remain copy-only. |

Prompt wall-time means (p95): original legacy 34.9 s (52.3 s), original adaptive 24.7 s (40.5 s),
held-out legacy 28.9 s (29.3 s), and held-out adaptive 29.5 s (39.7 s). Dictation rewrite means were
Natural 3.8 s, Casual 4.6 s, Formal 4.0 s, Concise 4.0 s and Unhinged 5.7 s; Clean averaged 14 ms.
No job exhausted its deadline. The new fallback recovered three complete copy-only outputs from
correction failures; the remaining four failures were all initial graph-validation failures. Full
outputs and per-sample metrics are in [quality-review-round-3.json](./quality-review-round-3.json).

### Assessment after Round 3 (historical; evaluation continued)

The 95% usability gate passed in Round 3, but the approved plan's complete quality gates did not:
the original prompt means, legacy held-out mean, Natural/Formal/Unhinged Dictation means,
approved-output floor, and critical-fidelity gate remained unmet. The subsequent request
superseded the original three-round limit; the measured desktop-deadline evaluation continued
below. Preserve copy-only behavior for unapproved drafts; do not lower the independent rubric or
treat reviewer approval as proof of quality. This evaluation is finite synthetic evidence, not a
production-quality guarantee.

## Expanded desktop-deadline evaluation

The following two additional batches use the same already-selected `qwen3.5-9b-q4km`, 60-second
desktop profile, temperature 0.3, top-k 40, top-p 0.9, min-p 0.05, 768-token cap, up to two
structural repairs, and three repetitions. They use synthetic fixed context, no history, and the
print-only inserter. No desktop process was stopped or restarted; native paste, personal history,
model switching, and model downloads were not used. Full per-sample outputs and operational data
are preserved in [quality-review-batch-1.json](./quality-review-batch-1.json) and
[quality-review-batch-2.json](./quality-review-batch-2.json).

The original 0–10 rubric above was applied independently to every sample. Failed or empty outputs
score 0; complete copy-only text is graded normally. Reviewer status and automatic-paste eligibility
were inspected only after grading. The tables list the three independent grades for each case.

### Desktop batch 1: baseline with two fresh held-outs

| Case | Legacy grades | Mean | Adaptive grades | Mean |
| --- | --- | ---: | --- | ---: |
| o01 explanation | 8.5, 8.5, 8.5 | 8.50 | 8, 7.5, 8 | 7.83 |
| o02 double-click fix | 9.5, 9.5, 4.5 | 7.83 | 0, 6.5, 0 | 2.17 |
| o03 low-cost research | 9.5, 8, 7.5 | 8.33 | 8.5, 8.5, 0 | 5.67 |
| o04 meeting-notes email | 8.5, 8.5, 7.5 | 8.17 | 9, 8.5, 9 | 8.83 |
| o05 independent evidence | 0, 0, 0 | 0.00 | 8.5, 0, 0 | 2.83 |
| o06 corrected French translation | 2.5, 2.5, 4.5 | 3.17 | 9, 4.5, 4.5 | 6.00 |
| o07 flaky API tests | 0, 0, 0 | 0.00 | 0, 8.5, 0 | 2.83 |
| o08 bedtime story | 8.5, 7, 7.5 | 7.67 | 8.5, 8.5, 8.5 | 8.50 |
| o09 image creation | 9.5, 8.5, 9 | 9.00 | 0, 9, 0 | 3.00 |

Original means: legacy **5.85/10**; adaptive **5.30/10**.

| Held-out case | Legacy grades | Adaptive grades |
| --- | --- | --- |
| h01 missing reference | 8.5, 8.5, 8.5 | 8.5, 8.5, 8.5 |
| h02 negation | 8.5, 9, 8.5 | 8.5, 8.5, 8.5 |
| h03 semantic double-click | 0, 4.5, 0 | 8.5, 0, 9 |
| h04 interactive tutoring | 9, 9, 9 | 9, 9, 9 |
| h05 translation | 8.5, 7.5, 8.5 | 8.5, 9, 8.5 |
| h06 evidence disagreement | 7.5, 9, 9 | 0, 0, 8.5 |
| h07 post-verification edit | 8.5, 8.5, 8.5 | 9, 8.5, 9 |
| h08 media creation | 8.5, 9, 8.5 | 8.5, 2, 9.5 |
| b1_h09 separate evidence | 0, 6.5, 9 | 0, 5.5, 5.5 |
| b1_h10 planned migration | 8.5, 8.5, 8.5 | 0, 8.5, 0 |

Held-out means: legacy **7.52/10**; adaptive **6.53/10**.

| Dictation tone | d01 grades | d02 grades | d03 grades | Mean |
| --- | --- | --- | --- | ---: |
| Clean transcript | 10, 10, 10 | 10, 10, 10 | 10, 10, 10 | 10.00 |
| Natural | 4.5, 4.5, 4.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 7.83 |
| Casual | 9, 4.5, 4.5 | 9, 9, 9 | 9.5, 9.5, 9.5 | 8.17 |
| Formal | 4.5, 4.5, 4.5 | 6.5, 6.5, 6.5 | 4.5, 4.5, 4.5 | 5.17 |
| Concise | 4.5, 9, 9 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 8.83 |
| Unhinged | 6.5, 6.5, 6.5 | 6.5, 6.5, 8.5 | 7.5, 7, 5.5 | 6.78 |

Batch 1 produced **144/168 usable (85.7%)**, below the 95% threshold. All 24 unusable samples
failed structural validation (`InvalidPrompt`); none exhausted the deadline. Outcomes were 50
inserted, 94 blocked/copy-only (89 rejected reviews, 2 unavailable reviews, and 3 outputs blocked
by destination capability after a checked/corrected review), and 24 failed. There were 50
auto-paste-eligible samples (29.8%). Among the 44 reviewer-checked/corrected samples, four scored
below 7: adaptive o02 #2, legacy h03 #2, and two Unhinged d02 samples. Two approved critical
failures were observed: adaptive o02 #2 replaced the double-click behavior with a per-click
requirement and a single-click test, and legacy h03 #2 declared final verification parallel with
writing the regression test. Prompt wall-time means/p95: original legacy 23.98/51.05 seconds,
original adaptive 23.72/36.82, held-out legacy 22.70/42.97, held-out adaptive 27.67/39.77.
Dictation wall-time mean/p95 (seconds): Clean 0.01/0.02, Natural 3.88/7.02, Casual 4.46/7.85,
Formal 3.96/7.81, Concise 3.99/7.71, Unhinged 5.44/8.52. No sample exhausted the 60-second
deadline.

### Desktop batch 2: graph-guidance refinement and two new held-outs (four cumulative)

| Case | Legacy grades | Mean | Adaptive grades | Mean |
| --- | --- | ---: | --- | ---: |
| o01 explanation | 8.5, 9, 9 | 8.83 | 9, 7.5, 8 | 8.17 |
| o02 double-click fix | 9.5, 9.5, 6.5 | 8.50 | 6.5, 8, 9 | 7.83 |
| o03 low-cost research | 7.5, 9, 7.5 | 8.00 | 9, 0, 0 | 3.00 |
| o04 meeting-notes email | 9, 8, 0 | 5.67 | 9, 9, 0 | 6.00 |
| o05 independent evidence | 8.5, 8.5, 0 | 5.67 | 8.5, 9, 8.5 | 8.67 |
| o06 corrected French translation | 9, 4.5, 9 | 7.50 | 8.5, 9, 9 | 8.83 |
| o07 flaky API tests | 8.5, 9, 8 | 8.50 | 8.5, 8.5, 9 | 8.67 |
| o08 bedtime story | 8.5, 8.5, 9 | 8.67 | 9, 9, 9 | 9.00 |
| o09 image creation | 8.5, 8.5, 8.5 | 8.50 | 8.5, 8.5, 8.5 | 8.50 |

Original means: legacy **7.76/10**; adaptive **7.63/10**.

| Held-out case | Legacy grades | Adaptive grades |
| --- | --- | --- |
| h01 missing reference | 9, 8.5, 8.5 | 6.5, 8.5, 9 |
| h02 negation | 8.5, 8.5, 9 | 8.5, 8.5, 8.5 |
| h03 semantic double-click | 9, 8.5, 9 | 8.5, 8.5, 4.5 |
| h04 interactive tutoring | 5.5, 8.5, 8.5 | 9, 9, 9 |
| h05 translation | 8.5, 8.5, 9 | 8.5, 9, 9 |
| h06 evidence disagreement | 9, 7.5, 0 | 8.5, 9, 8.5 |
| h07 post-verification edit | 8.5, 8.5, 9 | 9, 9, 9 |
| h08 media creation | 8.5, 8.5, 9 | 2, 8.5, 9 |
| b1_h09 separate evidence | 9, 8.5, 8.5 | 9, 6.5, 9 |
| b1_h10 planned migration | 9, 8.5, 8.5 | 9, 9, 9 |
| b2_h11 project update | 0, 8.5, 9 | 8.5, 4.5, 9 |
| b2_h12 quoted instruction | 8, 8.5, 6.5 | 9, 6.5, 8.5 |

Held-out means: legacy **8.00/10**; adaptive **8.18/10**.

| Dictation tone | d01 grades | d02 grades | d03 grades | Mean |
| --- | --- | --- | --- | ---: |
| Clean transcript | 10, 10, 10 | 10, 10, 10 | 10, 10, 10 | 10.00 |
| Natural | 4.5, 4.5, 9 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 8.33 |
| Casual | 9, 9, 9 | 9, 9, 9 | 9.5, 9.5, 9.5 | 9.17 |
| Formal | 4.5, 4.5, 7 | 6.5, 6.5, 6.5 | 4.5, 4.5, 7 | 5.72 |
| Concise | 4.5, 9, 9 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 8.83 |
| Unhinged | 6.5, 6.5, 6.5 | 6.5, 7, 8.5 | 6.5, 6.5, 5.5 | 6.67 |

Batch 2 produced **172/180 usable (95.6%)**, meeting the usability threshold; all eight unusable
samples failed structural validation (`InvalidPrompt`). Outcomes were 62 inserted, 110
blocked/copy-only (106 rejected reviews, 2 unavailable reviews, and 2 outputs blocked by
destination capability after a checked/corrected review), and 8 failed. There were 62
auto-paste-eligible samples (34.4%). Three of 55 reviewer-checked/corrected samples scored below
7: Unhinged d02 #1 and Unhinged d03 #1–2, all of which lacked the requested playful tone. No
approved critical fidelity change was found. Prompt wall-time means/p95: original legacy
23.11/39.79 seconds, original adaptive 22.05/35.95, held-out legacy 23.23/50.67, held-out
adaptive 24.15/33.80. Dictation wall-time mean/p95 (seconds): Clean 0.01/0.02, Natural 4.00/7.61,
Casual 4.56/7.91, Formal 4.03/7.78, Concise 4.11/8.06, Unhinged 5.34/8.05. No sample exhausted
the deadline.

### Expanded-batch gate status

| Gate | Batch 1 | Batch 2 |
| --- | --- | --- |
| Original prompt mean ≥8.5 for both policies | Fail: 5.85 legacy, 5.30 adaptive | Fail: 7.76 legacy, 7.63 adaptive |
| Held-out prompt mean ≥8.5 for both policies | Fail: 7.52 legacy, 6.53 adaptive | Fail: 8.00 legacy, 8.18 adaptive |
| Mean ≥8.5 for every Dictation tone | Fail: Natural, Casual, Formal, Unhinged | Fail: Natural, Formal, Unhinged |
| No reviewer-approved output below 7 | Fail: 4 | Fail: 3 |
| Zero critical fidelity violations among approved outputs | Fail: 2 | Pass: 0 |
| At least 95% usable | Fail: 85.7% | Pass: 95.6% |
| 60-second deadline exhausted | 0 | 0 |

The second batch materially improved structural usability and several prompt means, but neither
batch passes the complete quality gates, so they do not establish consistency. The final Dictation
guard for explicit negations and verbatim quotes, stronger tone instructions/review criteria, and
clearer verification dependencies were added after Batch 2 was built; these refinements will first
be measured in the next batch. Continue grading every result, including copy-only outputs and all
failures, with the same rubric and two consecutive passing batches required.

### Desktop batch 3: cumulative held-outs, dictation fidelity, and graph refinements

Run: the same selected `qwen3.5-9b-q4km`, desktop 60-second job deadline, temperature 0.3,
top-k 40, top-p 0.9, min-p 0.05, 768-token cap, up to two structural repairs, and three repetitions.
The 14 held-out requests include all prior held-outs and two fresh cases. The run contains 9
original requests × 2 policies × 3 repetitions, 14 held-outs × 2 policies × 3 repetitions, and
3 dictation inputs × 6 tones × 3 repetitions (192 samples total). It reused the selected model,
with no history, native paste, model switch, or download. The six cumulative fresh held-outs are
preserved in [quality-review-heldout-batch-3.toml](./quality-review-heldout-batch-3.toml), and the
complete per-sample outputs and runtime metrics are in
[quality-review-batch-3.json](./quality-review-batch-3.json).

The same independent rubric was applied to all final text, including blocked/copy-only outputs;
failed or empty results score 0. Scores below are the three repetitions in order.

| Original request | Legacy grades | Mean | Adaptive grades | Mean |
| --- | --- | ---: | --- | ---: |
| o01 explanation | 8, 8.5, 9 | 8.50 | 8, 8, 8 | 8.00 |
| o02 double-click fix | 9.5, 7.5, 9.5 | 8.83 | 6.5, 8.5, 8.5 | 7.83 |
| o03 low-cost research | 8.5, 7.5, 9 | 8.33 | 7.5, 6.5, 8.5 | 7.50 |
| o04 meeting-notes email | 8.5, 9, 7.5 | 8.33 | 9, 9, 7.5 | 8.50 |
| o05 independent evidence | 7.5, 6.5, 8.5 | 7.50 | 8.5, 7.5, 9 | 8.33 |
| o06 corrected French translation | 5.5, 4, 8 | 5.83 | 7.5, 4, 5.5 | 5.67 |
| o07 flaky API tests | 8.5, 8.5, 8.5 | 8.50 | 8.5, 8.5, 8.5 | 8.50 |
| o08 bedtime story | 8.5, 8.5, 6.5 | 7.83 | 8.5, 8.5, 8.5 | 8.50 |
| o09 image creation | 8.5, 8.5, 8.5 | 8.50 | 8.5, 8.5, 8.5 | 8.50 |

Original means: legacy **8.02/10**, adaptive **7.93/10**. The translation outputs still
occasionally omit either the requested email or the missing-source clarification. The approved
adaptive o03 output contains `(after 0)` in step prose; the approved legacy o08 output does too.
Both are malformed dependencies that the parser previously ignored.

| Held-out request | Legacy grades | Adaptive grades |
| --- | --- | --- |
| h01 missing reference | 9, 8.5, 8.5 | 8.5, 9, 9 |
| h02 negation | 8.5, 9, 8.5 | 9, 9, 8.5 |
| h03 semantic double-click | 6.5, 8.5, 7.5 | 8.5, 8.5, 4.5 |
| h04 interactive tutoring | 9, 5.5, 9 | 9, 9, 9 |
| h05 translation | 9, 8.5, 8.5 | 8.5, 8.5, 8.5 |
| h06 evidence disagreement | 9, 0, 7.5 | 9, 8.5, 8.5 |
| h07 post-verification edit | 8.5, 8.5, 9 | 9, 9, 9 |
| h08 media creation | 9, 9, 7.5 | 9, 8.5, 2 |
| b1_h09 separate evidence | 8.5, 8, 9 | 8.5, 8.5, 8.5 |
| b1_h10 planned migration | 8.5, 8.5, 8.5 | 8.5, 8.5, 9 |
| b2_h11 project update | 8, 8.5, 8.5 | 8.5, 0, 8.5 |
| b2_h12 quoted instruction | 8.5, 8.5, 8 | 8.5, 8.5, 8.5 |
| b3_h13 unconfirmed venue | 0, 6, 8.5 | 8.5, 8, 9 |
| b3_h14 persisted setting | 9, 9, 9 | 8.5, 7.5, 9 |

Held-out means: legacy **7.96/10** and adaptive **8.19/10**. Three held-out samples returned
no usable text: h06 legacy #2 and b3_h13 legacy #1 failed graph repair, while b2_h11 adaptive #2
also failed graph repair. The independently graded b3_h13 legacy #2 over-asks for organizer
details; b3_h14 adaptive #2 permits implementation and test authoring in parallel, which weakens
the test's value as a regression for that implementation. The h03 legacy #1 omission of the
explicit no-new-endpoint constraint is more serious: it was reviewer-approved and auto-paste
eligible.

| Dictation tone | d01 grades | d02 grades | d03 grades | Mean |
| --- | --- | --- | --- | ---: |
| Clean transcript | 10, 10, 10 | 10, 10, 10 | 10, 10, 10 | 10.00 |
| Natural | 4.5, 9.5, 4.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 8.39 |
| Casual | 9, 9, 9 | 8, 8, 9.5 | 9.5, 9.5, 9.5 | 9.00 |
| Formal | 4.5, 4.5, 4.5 | 6.5, 6.5, 6.5 | 7.5, 7.5, 4.5 | 5.83 |
| Concise | 9.5, 4.5, 4.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 8.39 |
| Unhinged | 6.5, 5, 6.5 | 5, 7, 6.5 | 5.5, 6.5, 5.5 | 6.00 |

Natural and Concise each lose the exact quoted paragraph in two d01 samples; those outputs were
copy-only. Formal outputs either omit the quote or merge explicit paragraph boundaries, all
remaining copy-only. Unhinged output either stays unchanged and misses the requested style, or
adds unsupported rhetorical material (including a new delivery implication and an invented
metaphor). The reviewer correctly blocks most deviations, but one unchanged d02 Unhinged sample
and one unchanged d03 Unhinged sample were accepted despite scoring 6.5.

#### Batch 3 gates and operational measures

| Gate | Result |
| --- | --- |
| Original prompt mean ≥8.5 for both policies | **Fail: 8.02 legacy, 7.93 adaptive** |
| Held-out prompt mean ≥8.5 for both policies | **Fail: 7.96 legacy, 8.19 adaptive** |
| Mean ≥8.5 for every Dictation tone | **Fail:** Natural 8.39, Formal 5.83, Concise 8.39, Unhinged 6.00; Clean 10.00 and Casual 9.00 |
| No reviewer-approved output below 7 | **Fail: 5** (adaptive o03 #2, legacy o08 #3, legacy h03 #1, Unhinged d02 #3, Unhinged d03 #2) |
| Zero critical fidelity violations among approved outputs | **Fail: 3** (two malformed `(after 0)` dependencies and the omitted no-new-endpoint exclusion) |
| At least 95% usable | **Pass: 189/192 (98.4%)** |
| 60-second deadline exhausted | **0/192** |
| Auto-paste eligible | **58/192 (30.2%)**; 131 blocked/copy-only and 3 invalid-prompt failures |

The 131 blocked outcomes comprise 128 quality-review-only and 3 unconfirmed-surface outcomes.
Review statuses were 39 checked, 13 corrected, 125 rejected, 3 unavailable and 12 without a
semantic review (9 Clean transcript, 3 failed graph repairs). There were 309 review calls and 134
targeted rewrite calls. Prompt wall-time mean/p95: original legacy 21.2/34.0 s, original adaptive
20.9/30.0 s, held-out legacy 21.1/38.5 s and held-out adaptive 24.5/32.0 s. Dictation mean/p95:
Clean 0.02/0.02 s, Natural 4.18/8.13 s, Casual 4.35/8.13 s, Formal 4.10/8.15 s, Concise
4.13/7.58 s and Unhinged 7.03/8.95 s. No call exceeded the job deadline.

The `(after 0)` samples predate the structural parser fix in
[structure.rs](../crates/promptify-core/src/structure.rs), which now rejects dependency metadata
embedded in step prose as well as headings; its regression and the full 219-test core suite pass.
Batch 3 remains an overall failure, not a passing predecessor. The next serialized run adds fresh
held-outs and measures the parser fix at the same desktop deadline; two consecutive complete
batches must still pass every quality gate.

### Desktop batch 4: explicit-exclusion and tone-prompt refinement

Run: the same already-selected `qwen3.5-9b-q4km` and desktop 60-second deadline, with temperature
0.3, top-k 40, top-p 0.9, min-p 0.05, 768-token cap, up to two structural repairs, and three
repetitions. The implementation now explicitly reminds generation and review to preserve every
explicit prohibition in the prompt, and the full per-tone instructions are also included in a
targeted Dictation rewrite. The 16 held-outs are cumulative, including fresh vendor-quote and
keyboard-repeat cases. No history, native paste, model switch, download, or desktop restart was
used. The complete 204-sample output is preserved in
[quality-review-batch-4.json](./quality-review-batch-4.json), with its eight cumulative additions
in [quality-review-heldout-batch-4.toml](./quality-review-heldout-batch-4.toml).

Every final sample, including copy-only text and empty failures, was independently graded with the
same rubric. Three-repetition vectors are in sample order.

| Original request | Legacy grades | Mean | Adaptive grades | Mean |
| --- | --- | ---: | --- | ---: |
| o01 explanation | 9, 9, 9 | 9.00 | 8.5, 8.5, 8.5 | 8.50 |
| o02 double-click fix | 9.5, 9.5, 9.5 | 9.50 | 9, 9, 9 | 9.00 |
| o03 low-cost research | 9, 7.5, 7.5 | 8.00 | 6.5, 0, 8 | 4.83 |
| o04 meeting-notes email | 8.5, 8.5, 0 | 5.67 | 9, 9, 9 | 9.00 |
| o05 independent evidence | 5, 5.5, 8.5 | 6.33 | 8, 8, 9 | 8.33 |
| o06 corrected French translation | 5.5, 5.5, 5.5 | 5.50 | 5.5, 3.5, 5.5 | 4.83 |
| o07 flaky API tests | 8.5, 7, 8.5 | 8.00 | 7, 5.5, 6.5 | 6.33 |
| o08 bedtime story | 9, 9, 9 | 9.00 | 8.5, 7.5, 9 | 8.33 |
| o09 image creation | 9, 9, 9 | 9.00 | 9, 8.5, 9 | 8.83 |

Original means: legacy **7.78/10** and adaptive **7.56/10**. A major fidelity failure
was reviewer-approved: legacy o05 #1 changed the explicitly unresolved disagreements into
"resolved disagreements." Legacy o04 #3 failed to return any text. The translation outputs
still omit at least one of the email/translation deliverables. Several other prompts add
unrequested test plans, documentation, or browsing assumptions.

| Held-out request | Legacy grades | Adaptive grades |
| --- | --- | --- |
| h01 missing reference | 8.5, 7.5, 7.5 | 7.5, 0, 8.5 |
| h02 negation | 6.5, 6.5, 9 | 8.5, 9, 9 |
| h03 semantic double-click | 9, 8.5, 9 | 9, 8, 9 |
| h04 interactive tutoring | 8.5, 0, 7.5 | 9, 8.5, 9 |
| h05 translation | 8.5, 8.5, 8.5 | 8.5, 8.5, 8.5 |
| h06 evidence disagreement | 9, 9, 8.5 | 8.5, 8.5, 8.5 |
| h07 post-verification edit | 9, 8.5, 8.5 | 8.5, 9, 8.5 |
| h08 media creation | 9, 9, 9 | 9, 9, 9 |
| b1_h09 separate evidence | 8, 6.5, 8.5 | 9, 6.5, 8.5 |
| b1_h10 planned migration | 8.5, 9, 9 | 9, 9, 9 |
| b2_h11 project update | 8.5, 8.5, 0 | 7.5, 8, 8.5 |
| b2_h12 quoted instruction | 8.5, 8.5, 8.5 | 7.5, 7.5, 7.5 |
| b3_h13 unconfirmed venue | 8.5, 7.5, 9 | 8.5, 8.5, 9 |
| b3_h14 persisted setting | 9, 9, 9 | 0, 9, 8 |
| b4_h15 conflicting vendor quotes | 8.5, 8.5, 8.5 | 0, 9, 7.5 |
| b4_h16 keyboard-repeat regression | 9, 9, 9 | 8.5, 8.5, 9 |

Held-out means: legacy **8.11/10**, adaptive **8.00/10**. One h02 legacy output changes a
request to draft an email into a request to send it; another repeats that scope change in a
large, invented context block. The adaptive b1_h09 #2 output makes independent analysis and
listing disagreements parallel, so comparison can start before both analyses are complete.

| Dictation tone | d01 grades | d02 grades | d03 grades | Mean |
| --- | --- | --- | --- | ---: |
| Clean transcript | 10, 10, 10 | 10, 10, 10 | 10, 10, 10 | 10.00 |
| Natural | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.50 |
| Casual | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.50 |
| Formal | 9, 9, 9.5 | 8, 8, 8 | 8, 8, 8 | 8.39 |
| Concise | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.50 |
| Unhinged | 4.5, 4.5, 4.5 | 4.5, 6.5, 6.5 | 7.5, 7.5, 7.5 | 5.94 |

The exact quotes and paragraph boundaries were preserved in all d01 rewrite outputs. Natural,
Casual, and Concise were faithful on these inputs. Formal was indistinguishable from the source on
the two already-neutral d02/d03 samples. Unhinged d01 materially changed "do not promise payment"
into skepticism that cash would arrive, and changed "send" to "drop off"; this was reviewer-approved
and pasted. Unhinged d02 #1 added "like magic" and an unsupported consequence, and was copy-only.
The two accepted d02 Unhinged samples remained unchanged and scored below 7. The d03 "give Sam a
ring" wording was safe but not sufficiently playful to meet the requested tone.

#### Batch 4 gates and operational measures

| Gate | Result |
| --- | --- |
| Original prompt mean ≥8.5 for both policies | **Fail: 7.78 legacy, 7.56 adaptive** |
| Held-out prompt mean ≥8.5 for both policies | **Fail: 8.11 legacy, 8.00 adaptive** |
| Mean ≥8.5 for every Dictation tone | **Fail:** Formal 8.39, Unhinged 5.94; Clean 10.00, Natural 9.50, Casual 9.50, Concise 9.50 |
| No reviewer-approved output below 7 | **Fail: 6** (o05 legacy #1; h02 legacy #1–2; Unhinged d01 #1 and d02 #2–3) |
| Zero critical fidelity violations among approved outputs | **Fail: 4** (o05 legacy #1, both h02 legacy scope changes, Unhinged d01 #1) |
| At least 95% usable | **Pass: 197/204 (96.6%)** |
| 60-second deadline exhausted | **0/204** |
| Auto-paste eligible | **67/204 (32.8%)**; 130 blocked and 7 invalid-prompt failures |

The 130 blocked outcomes comprise 125 quality-review-only, 4 unconfirmed-surface, and 1
unsupported-graph outcome. Review statuses were 41 checked, 22 corrected, 114 rejected,
11 unavailable, and 16 without review (9 Clean transcript and 7 failed graph repairs). There were
324 review calls and 143 targeted rewrites. Prompt wall-time mean/p95: original legacy 20.6/33.7 s,
original adaptive 23.2/38.2 s, held-out legacy 21.9/32.1 s, and held-out adaptive 27.2/35.0 s.
Dictation mean/p95: Clean 0.01/0.02 s, Natural 4.47/8.50 s, Casual 4.78/7.99 s, Formal
5.81/9.77 s, Concise 4.36/8.11 s, and Unhinged 6.56/9.88 s. No sample exhausted the original
60-second job deadline; the largest latency cost remains two semantic reviews plus one rewrite
when issues are found.

Batch 4 is not a passing batch, and Batches 3 and 4 do not establish consistent quality. In
particular, the local reviewer still approved explicit scope and meaning changes despite the
stronger cross-check instruction. Continue with the unchanged independent rubric and fresh
held-outs; neither rejection rate nor reviewer approval substitutes for the acceptance gates.

### Desktop batch 5: action-stage and unresolved-conflict refinement

Run: the same already-selected `qwen3.5-9b-q4km`, desktop 60-second deadline, decoding parameters,
token cap, and repair limit as Batch 4. This batch adds general instructions to preserve action
stage/certainty and keep user-designated disagreements unresolved; Unhinged guidance now retains
fact-bearing verbs and limits the flourish to language. The cumulative corpus contains 18 held-outs,
including the fresh draft-for-approval and conflicting-system-status cases. The complete
216-sample run is preserved in [quality-review-batch-5.json](./quality-review-batch-5.json), with
the cumulative held-out additions in [quality-review-heldout-batch-5.toml](./quality-review-heldout-batch-5.toml).
No user history or native paste was used, and no model was changed or downloaded.

All final text was graded independently of runtime reviewer verdicts, including every blocked
copy-only result; failures and incomplete text receive 0. The three scores in each cell follow
sample order.

| Original request | Legacy grades | Adaptive grades |
| --- | --- | --- |
| o01 explanation | 6, 6, 5.5 | 5.5, 5.5, 5.5 |
| o02 double-click fix | 8.5, 8.5, 8.5 | 8, 8, 7.5 |
| o03 low-cost research | 7.5, 7.5, 8.5 | 8.5, 8.5, 8.5 |
| o04 meeting-notes email | 6, 0, 0 | 7, 8, 7.5 |
| o05 independent evidence | 9, 9, 8.5 | 8.5, 8.5, 9 |
| o06 corrected French translation | 9, 4, 4 | 9, 9, 9 |
| o07 flaky API tests | 8.5, 7.5, 7.5 | 8.5, 8.5, 8.5 |
| o08 bedtime story | 8, 9, 9 | 8.5, 8.5, 8.5 |
| o09 image creation | 9, 9, 9 | 0, 8.5, 8.5 |

Original means: legacy **7.13/10**, adaptive **7.74/10**. All six simple explanation outputs
inflate a one-sentence question into an overbuilt graph with an extra summary or technical detail.
Legacy o04 #1 adds "ready to send" to a draft request; both remaining legacy jobs failed. Legacy
o06 #2–3 restore the retracted email request and require an email as well as the corrected
translation. Adaptive o09 #1 failed.

| Held-out request | Legacy grades | Adaptive grades |
| --- | --- | --- |
| h01 missing report | 8.5, 9, 8.5 | 7, 0, 0 |
| h02 draft email without lateness | 2, 9, 9 | 8.5, 8.5, 8.5 |
| h03 double-click fix | 7.5, 9, 8.5 | 8.5, 8.5, 7.5 |
| h04 interactive tutoring | 7, 8.5, 7 | 8.5, 8.5, 8 |
| h05 exact translation | 9, 9, 9 | 9, 8.5, 9 |
| h06 conflicting trial counts | 9, 8.5, 7.5 | 9, 8.5, 9 |
| h07 post-edit test and lint | 8.5, 8.5, 8.5 | 8.5, 9, 9 |
| h08 image creation | 9, 9, 9 | 3, 2, 8 |
| b1_h09 separate evidence | 8.5, 8.5, 8.5 | 6.5, 0, 0 |
| b1_h10 migration update | 9, 9, 8.5 | 8.5, 9, 9 |
| b2_h11 project status | 8.5, 8.5, 6.5 | 8.5, 8.5, 8.5 |
| b2_h12 quoted instruction | 9, 8.5, 8.5 | 7.5, 8.5, 9 |
| b3_h13 venue confirmation | 7, 6.5, 8.5 | 8.5, 8, 8 |
| b3_h14 saved setting | 9, 9, 9 | 9, 8.5, 9 |
| b4_h15 conflicting vendor quotes | 9, 8.5, 9 | 9, 8.5, 7.5 |
| b4_h16 Enter key repeat | 8.5, 9, 9 | 9, 9, 9 |
| b5_h17 draft for approval | 9, 9, 6.5 | 8.5, 9, 9 |
| b5_h18 unresolved backup status | 9, 9, 8.5 | 9, 9, 9 |

Held-out means: legacy **8.38/10**, adaptive **7.68/10**. Legacy h02 #1 reverses "do not set a
deadline" into "set a deadline"; it is blocked. Adaptive h08 #1–2 turn an explicit image-creation
request into instructions to generate a text/image prompt; both are blocked. Adaptive b1_h09 #2–3
and h01 #2 failed. Legacy b3_h13 #2 was auto-approved despite wording the task as "confirming the
venue" rather than asking the organizer to confirm it. The fresh h17 outputs mostly preserve draft
and approval status, but legacy #3 invents a timeline and a commitment to resolve the issue.

| Dictation tone | d01 grades | d02 grades | d03 grades | Mean |
| --- | --- | --- | --- | ---: |
| Clean transcript | 10, 10, 10 | 10, 10, 10 | 10, 10, 10 | 10.00 |
| Natural | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.50 |
| Casual | 8.5, 5.5, 5.5 | 9, 9, 9 | 9.5, 9.5, 9.5 | 8.33 |
| Formal | 6.5, 9, 9 | 9, 8, 8 | 9, 8.5, 8.5 | 8.39 |
| Concise | 9, 2, 9 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 8.56 |
| Unhinged | 5.5, 5.5, 3.5 | 5.5, 5.5, 5.5 | 5.5, 5.5, 5.5 | 5.28 |

Clean transcript preserved the corrected text and explicit paragraphs in all nine samples. Natural
also remained faithful in these already-natural inputs. Casual d01 #2–3 flatten the required
paragraph break. Formal d01 #1 also flattens paragraphs and removes the first-person sender.
Concise d01 #2 drops the entire exact quote and its instruction not to follow it. Unhinged did not
improve: eight of nine outputs simply echo neutral text; the ninth (d01 #3) changes "send" to
"dropping" and turns "do not promise payment" into "don't hold your breath for payment." The latter
was reviewer-approved and auto-paste eligible.

#### Batch 5 gates and operational measures

| Gate | Result |
| --- | --- |
| Original prompt mean ≥8.5 for both policies | **Fail: 7.13 legacy, 7.74 adaptive** |
| Held-out prompt mean ≥8.5 for both policies | **Fail: 8.38 legacy, 7.68 adaptive** |
| Mean ≥8.5 for every Dictation tone | **Fail:** Casual 8.33, Formal 8.39, Unhinged 5.28; Clean 10.00, Natural 9.50, Concise 8.56 |
| No reviewer-approved output below 7 | **Fail: 8** (legacy o04 #1, legacy b3_h13 #2, Unhinged d01 #3, all three Unhinged d02 outputs, and Unhinged d03 #2–3) |
| Zero critical fidelity violations among approved outputs | **Fail: 1** (Unhinged d01 #3 changes both the action and payment-negation force) |
| At least 95% usable | **Pass: 210/216 (97.2%)** |
| 60-second deadline exhausted | **0/216**; maximum sample wall time 47.7 s |
| Auto-paste eligible | **70/216 (32.4%)**: 32/162 Prompt samples and 38/54 Dictation samples |

There were 140 blocked outcomes (135 quality-review-only and 5 unconfirmed-surface) and 6
invalid-prompt failures. Review statuses: 47 checked, 19 corrected, 129 rejected, 6 unavailable,
and 15 without review (9 Clean transcript plus 6 failed jobs). The run made 340 review calls and
151 targeted rewrites. Prompt wall-time mean/p95: original legacy 19.3/26.2 s, original adaptive
23.5/37.5 s, held-out legacy 20.6/30.7 s, held-out adaptive 26.3/43.4 s. Dictation mean/p95:
Clean 0.01/0.02 s, Natural 4.43/8.52 s, Casual 4.09/8.23 s, Formal 5.77/8.31 s, Concise
4.33/8.43 s, and Unhinged 5.38/8.33 s. All samples completed within their original 60-second
deadline.

Batch 5 also fails the independent quality gates and does not establish a passing streak. The
reviewer blocked most otherwise complete Prompt candidates (only 32/162 were eligible), yet still
approved eight sub-7 results, including a critical Dictation fidelity failure. Do not interpret
the high usable-text rate as high quality or weaken the rubric; continue measuring the final output
itself against fresh cases.

### Desktop batch 6: proportional graph and non-literal Unhinged refinement

Run: the same installed `qwen3.5-9b-q4km`, desktop 60-second deadline and bounded decoding settings.
Generation guidance now calls for exactly two short graph steps on a simple one-intent request, no
unrequested preamble/output format, and a concise verification loop. Unhinged guidance explicitly
allows harmless non-literal wordplay while preserving fact-bearing verbs and negation. The
cumulative corpus has 20 held-outs, including the fresh one-sentence semaphore and internal-beta
cases. The full 228-sample output is in [quality-review-batch-6.json](./quality-review-batch-6.json);
the cumulative held-out file is [quality-review-heldout-batch-6.toml](./quality-review-heldout-batch-6.toml).
The selected model was reused; no history, native paste, model switch, or download was used.

Every final sample was independently graded with the fixed rubric; blocked complete text was not
discarded and failures received 0. Vectors are ordered by sample number.

| Original request | Legacy grades | Adaptive grades |
| --- | --- | --- |
| o01 explanation | 7.5, 8, 8 | 7.5, 7.5, 7.5 |
| o02 double-click fix | 8.5, 8.5, 9 | 8.5, 8.5, 8.5 |
| o03 low-cost research | 8.5, 8.5, 8.5 | 7.5, 8.5, 8.5 |
| o04 meeting-notes email | 9, 9, 9 | 7.5, 7.5, 9 |
| o05 independent evidence | 9, 9, 8.5 | 8.5, 8.5, 9 |
| o06 corrected French translation | 9, 9, 9 | 9, 9, 4 |
| o07 flaky API tests | 6.5, 8.5, 8.5 | 7.5, 8.5, 0 |
| o08 bedtime story | 9, 9, 8.5 | 8, 9, 8.5 |
| o09 image creation | 8.5, 8, 0 | 9, 9, 9 |

Original means: legacy **8.22/10**, adaptive **7.89/10**. The two-step direction reduced
overlength on some simple cases, but did not lift either policy to the gate. Adaptive o06 #3 again
restores the retracted email request, and adaptive o07 #3 failed. Legacy o07 #1 adds irrelevant
conflict-preservation instructions to a flaky-test repair.

| Held-out request | Legacy grades | Adaptive grades |
| --- | --- | --- |
| h01 missing report | 9, 9, 9 | 9, 9, 8.5 |
| h02 draft email without lateness | 9, 8.5, 8.5 | 8.5, 8.5, 8.5 |
| h03 double-click fix | 8.5, 9, 8.5 | 8.5, 8.5, 8.5 |
| h04 interactive tutoring | 9, 8.5, 7.5 | 8.5, 8.5, 9 |
| h05 exact translation | 9, 9, 9 | 0, 8.5, 9 |
| h06 conflicting trial counts | 8.5, 8.5, 9 | 8.5, 9, 8.5 |
| h07 post-edit test and lint | 9, 9, 8.5 | 9, 9, 9 |
| h08 image creation | 7.5, 9, 9 | 9, 9, 9 |
| b1_h09 separate evidence | 8.5, 8, 8.5 | 8.5, 8.5, 8.5 |
| b1_h10 migration update | 9, 9, 8.5 | 0, 9, 9 |
| b2_h11 project status | 9, 9, 8.5 | 0, 8.5, 8 |
| b2_h12 quoted instruction | 7.5, 7.5, 9 | 8, 8.5, 8.5 |
| b3_h13 venue confirmation | 9, 9, 9 | 9, 8.5, 9 |
| b3_h14 saved setting | 8.5, 9, 8 | 9, 8.5, 6.5 |
| b4_h15 conflicting vendor quotes | 6.5, 8.5, 9 | 8.5, 8.5, 8.5 |
| b4_h16 Enter key repeat | 9, 9, 9 | 9, 8.5, 6.5 |
| b5_h17 draft for approval | 7.5, 9, 9 | 8.5, 8, 7.5 |
| b5_h18 unresolved backup status | 9, 8.5, 9 | 9, 9, 9 |
| b6_h19 one-sentence explanation | 7, 9, 9 | 7.5, 8.5, 8.5 |
| b6_h20 internal beta status | 9, 9, 9 | 9, 8.5, 9 |

Held-out means: legacy **8.64/10**, adaptive **8.16/10**. Legacy b4_h15 #1 adds annual-total calculations and forbids a vendor recommendation the user did not ask for. Adaptive b3_h14 #3 and b4_h16 #3 wrongly mark test creation parallel with the implementation edit. Adaptive h05 #1 and b1_h10 #1 failed; adaptive b2_h11 #1 failed. The h19 simple case usually meets its one-sentence/no-code/no-analogy contract, but one legacy output still asks an unnecessary language question and one adaptive output repeats the request before the graph.

| Dictation tone | d01 grades | d02 grades | d03 grades | Mean |
| --- | --- | --- | --- | ---: |
| Clean transcript | 10, 10, 10 | 10, 10, 10 | 10, 10, 10 | 10.00 |
| Natural | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.50 |
| Casual | 9.5, 9.5, 5.5 | 9, 9, 9 | 9, 9.5, 9.5 | 8.83 |
| Formal | 9, 6.5, 9 | 6.5, 8, 8 | 8.5, 8.5, 8.5 | 8.06 |
| Concise | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.5, 9.5, 9.5 | 9.50 |
| Unhinged | 5.5, 1, 5.5 | 5, 5, 5 | 5, 4.5, 5 | 4.61 |

Clean transcript and Natural preserved all facts, corrected text, and paragraph boundaries in these
samples. Casual d01 #3 loses the required paragraph break. Formal d01 #2 turns the sender's active
"I will send" into passive "12 invoices will be sent"; Formal d02 #1 also flattens the explicit
paragraph boundary. Unhinged now adds more visible humor but remains far below target: d01 #2
invented a "financial wizard" identity and a "rule-abiding robot" identity, despite the request not
to follow the quoted instruction. That result was reviewer-approved and auto-paste eligible. The
d02/d03 humorous additions were rejected, but independently scored for fidelity and tone.

#### Batch 6 gates and operational measures

| Gate | Result |
| --- | --- |
| Original prompt mean ≥8.5 for both policies | **Fail: 8.22 legacy, 7.89 adaptive** |
| Held-out prompt mean ≥8.5 for both policies | **Fail: 8.64 legacy, 8.16 adaptive** |
| Mean ≥8.5 for every Dictation tone | **Fail:** Formal 8.06, Unhinged 4.61; Clean 10.00, Natural 9.50, Casual 8.83, Concise 9.50 |
| No reviewer-approved output below 7 | **Fail: 2** (Formal d01 #2 and Unhinged d01 #2) |
| Zero critical fidelity violations among approved outputs | **Fail: 1** (Unhinged d01 #2 adds invented identities/claims around the quoted instruction) |
| At least 95% usable | **Pass: 223/228 (97.8%)** |
| 60-second deadline exhausted | **0/228**; maximum sample wall time 39.9 s |
| Auto-paste eligible | **74/228 (32.5%)**: 38/174 Prompt samples and 36/54 Dictation samples |

There were 149 blocked outcomes (144 quality-review-only, 2 unsupported-graph, and 3
unconfirmed-surface) and 5 invalid-prompt failures. Review statuses: 44 checked, 26 corrected,
138 rejected, 6 unavailable, and 14 without review (9 Clean transcript plus 5 failed jobs). The
run made 362 review calls and 163 targeted rewrites. Prompt wall-time mean/p95: original legacy
19.3/35.1 s, original adaptive 22.5/32.0 s, held-out legacy 19.5/29.2 s, and held-out adaptive
24.8/33.2 s. Dictation mean/p95: Clean 0.01/0.02 s, Natural 4.37/8.41 s, Casual 4.31/8.17 s,
Formal 5.96/8.73 s, Concise 3.91/8.88 s, and Unhinged 9.26/10.68 s. No job exhausted the
original 60-second deadline.

Batch 6 still fails three independent quality categories and cannot form a consecutive passing
pair with any previous batch. Despite two distinct generalizable guidance refinements, the
unchanged local model still both overbuilds/omits details in Prompt samples and generates new
claims in Unhinged dictation; its reviewer also approved a critical instance. Do not treat the
usability or deadline results as a quality pass.

#### Stop point and remaining acceptance

The six expanded batches used the same selected local model and the real 60-second desktop
deadline. No two consecutive complete batches passed all quality gates. Batch 6 meets the
usability and deadline limits, but misses Prompt and Dictation mean thresholds, has two
reviewer-approved outputs below 7, and includes a critical approved fidelity failure. The
generalizable guidance refinements improved specific failure modes but did not yield stable
quality across policies, tones, and held-out inputs. Further prompt-only iteration under the
fixed-model constraint has diminishing returns and risks trading one fidelity failure for
another; this is an evidence-based blocker for the requested quality target, not proof that the
model can never meet it. The acceptance criteria remain unmet. Reopening model or product
constraints would require explicit authorization; no model switch or download is performed
automatically.
