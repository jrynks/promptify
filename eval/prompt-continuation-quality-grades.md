# Prompt continuation quality: bounded refinement workflow

## Criteria and independent grading

This workflow resumes the existing generated **Prompt** quality work, not Dictation redesign.
The acceptance criteria are unchanged: original and held-out means at least **8.5/10 for each
default (`legacy`) and adaptive policy**, at least **three samples per request**, no
reviewer-approved output below **7/10**, **zero observed critical fidelity violations** in
reviewer-approved outputs, at least **95% complete usable output** (including complete
review-only text, reported separately from automatic eligibility), and **two consecutive full
passing batches**. At most **three new refinement rounds** are allowed. Hitting the round
limit without meeting the criteria is failure, not completion of the quality goal.

Each final candidate is read independently of the local model's review verdict. Five dimensions
are scored separately out of 5: fidelity (intent, actions, facts and exclusions), dependency
coherence and requested independence, correction and complete post-edit rechecking, completion
criteria, and proportionality. The sum out of 25 is multiplied by 10/25. Failed or incomplete
outputs receive five zeroes. Complete blocked outputs remain in the denominator and are graded.
This formalizes the existing Prompt rubric; it does not add case-specific runtime validation or
relax any gate. The grader is Copilot reading actual outputs, **not blinded human grading**.
The saved tuples are explicit per-sample judgements, not heuristics derived from reviewer status,
keywords, or model self-scores.

Raw prior artifacts are unchanged. This report's dimension-based regrading of the compact batch
is the matched comparison baseline; historical batch-6 scores used the older holistic reporting
format and are contextual, not a paired dimension-based experiment.

## Saved compact batch 7: established baseline

Evidence: [actual outputs](quality-review-batch-7.json), [earlier ungraded snapshot](quality-review-batch-7-intermediate.json),
[independent per-sample five-dimensional grades](quality-review-batch-7-prompt-grades.json).
All 186 final Prompt samples were read and graded; the intermediate snapshot is retained but is
not mixed into the comparison.

| Group | Independent mean /10 |
| --- | ---: |
| Original default | 7.081 |
| Original adaptive | 8.504 |
| Held-out default | 8.242 |
| Held-out adaptive | 7.794 |

175/186 (94.1%) have complete usable text; **120/186 (64.5%)** are auto-paste eligible.
134 are reviewer-approved (`checked` or `corrected`), including generator/surface-blocked
results. **13 reviewer-approved samples score below 7**, and **one observed approved critical
fidelity failure** remains. Compact instructions therefore did not establish a passing batch.

The critical example and a distinct delivery-verification weakness are:

* `o02`, default sample 2: the user asks to fix two requests caused by clicking twice, add a
  regression test, run relevant tests, and leave the API unchanged. The approved output says
  **“only one request is sent per click event”** and tests **“a single click”**. It misses the
  failure trigger and can preserve the defect. Dimensions **2/4/4/3/3**, **6.4/10**.
* `h02`, default sample 2: **“Send the email to Morgan”** and completion requiring Morgan to
  receive it are not followed by a delivery check. Dimensions **5/4/3/3/4**, **7.6/10**.
  Because the request says “ask … by email” rather than explicitly “draft” or “do not send,”
  this is **not** counted as a critical fidelity violation; the verification defect remains.

Other failures include restoring the retracted email task instead of French translation,
declaring receipt of a missing report “done” instead of performing the requested summary and
comparison, repairing only lint without rerunning tests after the correction, and copying
unrequested format/recommendation and interactive boilerplate from teaching examples.

Prompt operational totals: **76 structural repair attempts**, **211 semantic review calls**,
**48 targeted rewrite calls**; 118 checked, 16 corrected, 30 rejected, 11 unavailable and
11 failed without review. There are 41 quality-review blocks, 12 unconfirmed-surface blocks
and two unsupported-graph blocks. All Prompt samples use the desktop 60-second deadline.

| Baseline group | Mean wall seconds | p95 seconds | Maximum seconds |
| --- | ---: | ---: | ---: |
| Original default | 13.56 | 21.66 | 27.36 |
| Original adaptive | 13.26 | 21.61 | 22.75 |
| Held-out default | 11.81 | 24.11 | 29.38 |
| Held-out adaptive | 12.31 | 20.98 | 24.60 |

## Reproducible evaluation boundary

All new runs use the already-selected installed **`qwen3.5-9b-q4km`**, temperature **0.3**,
top-k **40**, top-p **0.9**, min-p **0.05**, and incrementing worker-request-ID seed.
No model switch or download occurs. Each job retains **60 seconds total**, **768 generation
tokens**, a **6,000-character cap**, at most **two structural repairs**, and at most **one
semantic rewrite and two reviews**. The same real end-to-end orchestrator uses synthetic
`FixedContext`, `NoHistory`, and `PrintInserter`. The running desktop app is not restarted.

`NoHistory` returns only a fixture explicitly supplied in the evaluation TOML when requested;
it never reads or writes the user's history. No microphone, screen text or native paste is used.
The report saves that fixture alongside its request. `--prompt-only` skips Dictation samples;
it changes evaluation selection only, not product behavior or safety gates.

The fixed new corpus contains the nine originals, all 22 cumulative held-outs from batch 7,
and four new held-outs. Each has three samples under both policies: **210 Prompt samples
per full batch**. New cases test exactly “resume iterating on prompt quality now” with no
reference and with legitimate synthetic prior criteria, requested independent parallel evidence
analysis, and rerunning both regression and lint after every correction. These are never
referenced by production IDs or matched against their text.

```sh
export VULKAN_SDK=/usr LIBCLANG_PATH=/usr/lib64
PROMPTIFY_EVAL_SHOW=1 "${CARGO_TARGET_DIR:-target}/debug/promptify-cli" \
  eval-quality eval/quality-review.toml --prompt-only \
  --samples 3 --heldout-samples 3 --deadline-seconds 60 \
  --fresh-heldout eval/quality-review-heldout-prompt-rounds.toml \
  --output eval/quality-review-prompt-round-1.json \
  > eval/quality-review-prompt-round-1.log 2>&1
```

Seeds are deterministic per worker request, **not paired per case** when different repair/review
call counts change subsequent request IDs. Same-model review is an imperfect safety filter,
not independent proof of quality. “Inserted” in synthetic reports means print-only dispatch,
never a native paste.

## New round 1: targeted shared teaching

Generation guidance now preserves continuation as existing work with its criteria, retaining
an unresolved reference when context is absent rather than inventing a project/history or
reducing it to editing one draft. The shared graph contract requires the reported failure trigger
(not an easier scenario), combines drafting actions, rechecks all affected tests after corrections,
and distinguishes actual completion from receipt of inputs. Examples no longer teach invented
table/heading formats; coding examples rerun executable checks at their loop target.
The Prompt reviewer additionally checks continuation, multi-event versus per-event behavior,
and all post-correction outcomes. Dictation guidance is unchanged.

The immutable source snapshot and hashes are in
`quality-review-prompt-round-1-instructions.json`. [All 210 actual samples](quality-review-prompt-round-1.json)
and [independent grades](quality-review-prompt-round-1-grades.json) are retained, along with the
rejected-repair log. This full run took 59.6 minutes of summed sample wall time.

| Round 1 group | Mean /10 | Baseline-matched cases only /10 |
| --- | ---: | ---: |
| Original default | 7.156 | 7.156 |
| Original adaptive | 8.637 | 8.637 |
| Held-out default | 7.805 | 8.055 |
| Held-out adaptive | 8.113 | 8.358 |

Matched held-outs exclude the four new continuation/parallel/correction cases **only for the
baseline comparison**, never for the acceptance gates. Original default remains below threshold;
both full held-out groups remain below threshold. **198/210 usable (94.3%)** fails yield;
**69/210 (32.9%)** are auto-paste eligible. **Four reviewer-approved samples score below 7**,
and **three observed approved critical fidelity failures** remain:

* Adaptive `o02` #1 still fixes/completes **“one request per click”**, although its test says
  double-click. **2/5/4/2/4 = 6.8/10**. It is reviewer-approved but surface-blocked, and is
  correctly counted as an approved fidelity failure, not excused because it cannot auto-paste.
* Default `b4_h16` #2 invents the **“Copilot Chat extension”** as the implementation project.
  The request only supplies a keyboard bug; the destination is not the subject project.
  **1/5/4/3/3 = 6.4/10**, approved and eligible.
* Default `b7_h22` #1 drops conditional availability and asserts the invite has not been sent,
  despite only being told not to say it has been sent. **2/5/3/3/3 = 6.4/10**, approved and eligible.

Default `b1_h10` #1 is the fourth approved sub-7 candidate: it literally outputs `V` in Done
and copied rules rather than concrete completion (**5/5/3/2/2 = 6.8/10**).

The new continuation probes expose a particularly important failure. Without context, default
`c01` #1 hallucinates prior **pricing options, risks, testing and recommendation** copied from its
example (**0/4/3/2/2 = 4.4/10**, rejected). Adaptive `c01` outputs generic single-draft
editing, or asserts no prior prompt exists and invents a placeholder template. With explicit
synthetic previous work, default `c02` retains much of the real workflow but mishandles sample
counts/dimensions/completion; all three adaptive `c02` candidates still reduce the work to
editing a prompt draft (**1/4/3/2/3 = 5.2/10** each). These are actual outputs, not conclusions
based only on instruction text.

For contrast, new `c04` default #2 actually carries empty-selection regression plus lint through
the same final check and correction target:

> Step 2 (after 1): Implement the fix for the export bug and write the regression test that
> specifically triggers an empty selection scenario, ensuring it does not use a populated
> export as a substitute.
>
> Step 3 (after 2): Execute the regression test and the linting process to verify the fix and code quality.
>
> Loop: if Step 3 fails the stated checks (regression test passes and lint succeeds), return to
> Step 2 to correct the cause of the failure; then recheck Step 3 (max 2 rounds).

The complete output's dimensions are **5/5/5/5/4 = 9.6/10**, approved and eligible. The good
case does not offset the observed fidelity failures or establish a passing batch.

Round 1 operational totals: **100 structural repairs**, **263 semantic reviews**, **124 rewrites**;
63 checked, 15 corrected, 102 rejected, 18 unavailable and 12 failed without review. There
are 120 quality-review blocks and nine surface-unconfirmed blocks. No deadline is exhausted;
the maximum wall time is 48.81 seconds.

| Round 1 group | Mean wall seconds | p95 seconds | Maximum seconds |
| --- | ---: | ---: | ---: |
| Original default | 19.09 | 27.28 | 41.20 |
| Original adaptive | 15.72 | 28.97 | 32.63 |
| Held-out default | 17.92 | 35.54 | 48.81 |
| Held-out adaptive | 15.87 | 25.93 | 35.09 |

## New round 2: replace misleading teaching rather than append more rules

Based on those actual failures, this round replaces the variable/“generic success” loop teaching
with a concrete loop whose numbers/actions must be adapted. `Done when` must name verified
deliverables and constraints. Defect tests are explicitly conditional on coding defects, not
status notes or emails. Independent analyses have no mutual prerequisites. `<previous_prompt>`
is identified explicitly as continued work/constraints/criteria, not automatically a draft to edit;
examples are never previous work, and destination labels are not project facts.

The long pricing example that contaminated a no-context continuation is replaced by a concise
two-step announcement example. Default generation reuses the **existing general final-request
correction normalizer already used by adaptive**, with a regression for an unrelated corrected
poem/agenda request and a quoted correction. There is no case-ID or benchmark-text matching.
The shared instruction budget remains at most 240 words. All sampling/deadline/repair limits
and the full 210-sample corpus remain unchanged.

The second source snapshot is `quality-review-prompt-round-2-instructions.json`. Evaluation
now also flushes every complete synthetic sample as `sample-json:` in the raw log, so a later
interruption cannot erase already-produced output. This logging does not change generation.

All [210 actual outputs](quality-review-prompt-round-2.json) and their
[independent grades](quality-review-prompt-round-2-grades.json) are retained. This round took
58.3 minutes of summed sample wall time and **regressed**, despite fixing the retracted-task
case and conditional-invite case. No failed request was removed.

| Round 2 group | Mean /10 | Baseline-matched cases only /10 |
| --- | ---: | ---: |
| Original default | 8.119 | 8.119 |
| Original adaptive | 6.444 | 6.444 |
| Held-out default | 6.431 | 7.079 |
| Held-out adaptive | 6.379 | 6.527 |

Only **161/210 (76.7%)** yield complete usable text, and **66/210 (31.4%)** are auto-eligible.
There are **two approved samples below 7** and **four approved critical fidelity failures**:

* Adaptive `o02` #3 fixes/completes a **per-click** guarantee, not the two-click operation:
  **3/5/4/2/4 = 7.2/10**. It is approved but surface-blocked, still counted.
* Default `h01` #2 substitutes clarification questions and receipt of answers for the requested
  summary/comparison/citations: **1/5/3/2/2 = 5.2/10**, approved.
* Default `b5_h17` #2 says **“duplicate charge on ChatGPT (chatgpt.com in Chrome)”**:
  destination invented as the charge's subject. **2/5/4/4/3 = 7.2/10**, approved.
* Adaptive `c04` #2 begins **“Reproduce … by running the regression test”** and never creates
  the explicitly requested new regression test. The assumed artifact replaces a required action:
  **3/5/4/4/3 = 7.6/10**, approved.

The other approved sub-7 output is adaptive `h01` #2 (**3/5/3/3/3 = 6.8/10**).
The new continuation without context fails all default samples by inventing the help-center
announcement from the replacement example; all adaptive samples still edit one prompt's format.
With legitimate prior context, default `c02` #3 finally retains the actual quality workflow and
criteria (**5/5/5/4/4 = 9.2/10**, review unavailable), but two default samples fail entirely and
all three adaptive samples ignore the supplied workflow.

For a measured improvement on the original correction case, input `o06` is the corrected request
for French translation, not the retracted email. All three default outputs now preserve the
translation (**5/5/5/5/5 = 10/10**). Likewise all six `b7_h22` outputs now retain Thursday
availability conditional on the updated invite, without inventing sent status or agreeing to
Friday (**5/5/5/5/4 = 9.6/10** each). These isolated gains do not cancel the full-batch regression.

Round 2 operational totals: **144 structural repairs**, **192 semantic reviews**, **94 rewrites**;
62 checked, eight corrected, 82 rejected, nine unavailable and **49 failed without final output**.
There are 91 quality-review blocks and four surface-unconfirmed blocks; no deadline is exhausted.
The raw failures show malformed prerequisite/parallel annotations and unusable loops, not merely
reviewer disapproval. Abstract `(parallel with N)` teaching also causes unsupported parallelism
between diagnosis and fixes, or source analysis and dependent synthesis.

| Round 2 group | Mean wall seconds | p95 seconds | Maximum seconds |
| --- | ---: | ---: | ---: |
| Original default | 15.83 | 28.75 | 35.86 |
| Original adaptive | 15.26 | 24.09 | 26.09 |
| Held-out default | 17.27 | 34.57 | 36.59 |
| Held-out adaptive | 16.83 | 28.29 | 32.13 |

## New round 3: final permitted refinement

The third round replaces variable parallel syntax with a concrete two-independent-source
example, requiring synthesis after both analyses and prohibiting parallel work that needs
another step's results. Shared paired continuation examples contrast legitimate prior import
repair work with an absent reference: actually continue the supplied work/checks, or preserve
the unresolved reference and wait for essential context, never invent a project or call input
receipt completion. These generic examples are identical in default and adaptive generation;
cache-prefix counts and related tests include them.

Adaptive `code.implement` teaching now distinguishes an operation containing repeated events
from a per-event guarantee, requiring the complete event sequence and final implementation
checks. The selected original defect uses that task, so changing only `code.debug` would have
missed it. This is general task teaching, not transcript/ID matching. Shared prose remains
**240 words**, decoding/model/deadline/safety limits are unchanged, and the same 210-sample
corpus is locked. The third immutable snapshot is
`quality-review-prompt-round-3-instructions.json`.

The run was not restarted, and instructions were not adjusted while it ran. Afterward, all five
snapshot hashes still match the source, and the CLI binary was built after the last
instruction edit. The process exited after saving all 210 samples. The log has no panic. The
report records `qwen3.5-9b-q4km` with no model change or download, the desktop 60-second deadline,
768 tokens, 6,000 characters, two structural repairs, no history access, and no native paste.

All [210 actual outputs](quality-review-prompt-round-3.json) and their
[independent grades](quality-review-prompt-round-3-grades.json) are retained. Every complete
output was read, including blocked and review-unavailable text. The grade vectors in
[`grade-prompt-continuation.py`](grade-prompt-continuation.py) are explicit per-sample readings.
The script only publishes them and checks that failed samples are zeroed. It derives no score
from reviewer status, keywords, or model self-assessment. Regenerating with the script left
the baseline, round-1 and round-2 grade files byte-identical.

| Round 3 group | Mean /10 | Baseline-matched cases only /10 |
| --- | ---: | ---: |
| Original default | 7.644 | 7.644 |
| Original adaptive | 7.067 | 7.067 |
| Held-out default | 7.841 | 8.273 |
| Held-out adaptive | 7.764 | 8.285 |

All four groups are below 8.5. **182/210 (86.7%)** are usable, which is below 95%, and
**67/210 (31.9%)** are auto-eligible. There are **two approved samples below 7** and **six
approved critical fidelity failures**:

* Default `o02` #1 and #3 (**2/5/4/2/4 = 6.8/10** each): both are approved and eligible. #1 says
  to **“ensure exactly one request is sent per click”**, and its Done clause requires “one
  request per click.” Two clicks therefore still send two requests. Adaptive `o02` #2
  (**7.2/10**) is approved but surface-blocked, and it repeats the per-click guarantee. All
  six `o02` samples contain the defect, despite this round's event-sequence teaching.
* Default `h03` #1 (**2/5/4/3/4 = 7.2/10**) is approved and eligible. It implements a guard so
  **“exactly one request is sent per click,”** which contradicts the double-click requirement.
* Adaptive `c04` #1 and #2 (**8.0 and 7.6/10**) are approved but surface-blocked. They run
  “the regression test for the empty selection” but **never add the requested test**,
  repeating round 2's missing-artifact failure.

All six samples of two cases fail without usable output: the separate notes/spreadsheet review
(`b1_h09`) and the new parallel recordings/crash-log analysis (`c03`). This happened despite
round 3's new two-source example. All three default fraction-tutoring samples (`h04`) also fail.
All 28 failures are `InvalidPrompt`.

Continuation remains unsolved. With no context, no `c01` sample keeps the reference
unresolved and waits for the missing context. Default #1 invents **“Optimize the prompt for ChatGPT
(chatgpt.com in Chrome)”** from the destination label. The other samples assume an existing
prompt draft, session history, or “shared contract.” With legitimate synthetic previous work,
default `c02` #2 and #3 retain the real workflow and gates (**8.4 and 8.0/10**), but #1 fails.
All three adaptive `c02` samples again discard the supplied workflow and edit a single prompt
(**4.4–6.0/10**). Each is rejected, which does not change their fidelity grades.

Measured gains are real but isolated. All six `h07` outputs rerun **both tests and lint** after
the final edit and after any correction. For example, default #1 has “Step 5 (after 4): Rerun
both the test suite and the linter,” and its loop returns to the code edit before rechecking
Step 5 (**9.6/10**). In batch 7, the same request scored 6.4–7.6. All three default `c04`
outputs carry the empty-selection test plus lint through the final recheck (**9.73/10** mean).
Default `o06` keeps the corrected French request. Adaptive `o06` #2 still restores “Write an
email about the delay” in its preamble (rejected).

Round 3 operational totals: **132 structural repairs**, **241 semantic reviews** and
**103 rewrites**. Quality statuses are 66 checked, eight corrected, 91 rejected,
17 unavailable and 28 failed without output. There are 108 quality-review blocks and seven
surface-unconfirmed blocks. Seven reviewer-approved outputs are not eligible because of the
surface. **No deadline is exhausted**, but tail latency is the worst of the rounds: the maximum
is 51.32 seconds against the 60-second job deadline. The run took 61.8 minutes of summed sample
wall time.

| Round 3 group | Mean wall seconds | p95 seconds | Maximum seconds |
| --- | ---: | ---: | ---: |
| Original default | 19.59 | 38.13 | 43.73 |
| Original adaptive | 19.02 | 34.62 | 43.83 |
| Held-out default | 16.34 | 31.71 | 51.32 |
| Held-out adaptive | 17.84 | 31.11 | 49.67 |

## Comparison across the bounded rounds

| Measure | Batch 7 baseline | Round 1 | Round 2 | Round 3 |
| --- | ---: | ---: | ---: | ---: |
| Original default mean | 7.081 | 7.156 | 8.119 | 7.644 |
| Original adaptive mean | 8.504 | 8.637 | 6.444 | 7.067 |
| Held-out default mean | 8.242 | 7.805 | 6.431 | 7.841 |
| Held-out adaptive mean | 7.794 | 8.113 | 6.379 | 7.764 |
| Usable | 175/186 (94.1%) | 198/210 (94.3%) | 161/210 (76.7%) | 182/210 (86.7%) |
| Auto-eligible | 120/186 (64.5%) | 69/210 (32.9%) | 66/210 (31.4%) | 67/210 (31.9%) |
| Approved below 7 | 13 | 4 | 2 | 2 |
| Approved critical fidelity | 1 | 3 | 4 | 6 |
| Reviews / rewrites / repairs | 211 / 48 / 76 | 263 / 124 / 100 | 192 / 94 / 144 | 241 / 103 / 132 |
| Rejected / unavailable / failed | 30 / 11 / 11 | 102 / 18 / 12 | 82 / 9 / 49 | 91 / 17 / 28 |
| Deadline exhausted | 0 | 0 | 0 | 0 |
| Maximum wall seconds | 29.38 | 48.81 | 36.59 | 51.32 |

Batch 7 has 186 samples on the earlier 31-case corpus. Rounds 1–3 use the locked 35-case,
210-sample corpus. Its baseline-matched held-out means are 8.055/8.358, 7.079/6.527 and
8.273/8.285 (default/adaptive) in rounds 1–3, versus 8.242/7.794 in batch 7.

Mean independent grade of the new held-out cases (default / adaptive):

| Case | Round 1 | Round 2 | Round 3 |
| --- | ---: | ---: | ---: |
| `c01` continuation, no context | 5.07 / 5.20 | 5.20 / 6.27 | 6.67 / 5.87 |
| `c02` continuation, supplied previous work | 6.27 / 5.20 | 3.07 / 5.60 | 5.47 / 5.20 |
| `c03` requested independent parallel analysis | 5.07 / 7.20 | 0.00 / 5.33 | 0.00 / 0.00 |
| `c04` correction with full recheck | 9.33 / 9.47 | 3.20 / 5.07 | 9.73 / 8.53 |

## Final verdict

**The Prompt quality goal is not met.** None of the three permitted refinement rounds, nor
the baseline, passes one full batch. Each misses all four mean gates, the 95% usable gate, the
no-approved-sub-7 gate and the no-approved-critical-fidelity gate. Two consecutive passing
batches were therefore not achieved. The round limit is exhausted, so no fourth round was run.
This round-limited result is a failure, not completion.

Round 3 partly recovered from round 2's structural collapse. Usable output rose from 76.7% to
86.7%, and matched held-out means improved. It still did not beat round 1 on usability and is
worse on approved critical failures. Same-model review rejects many good outputs while approving
the critical `o02`, `h03` and `c04` failures. Auto-paste eligibility therefore stays near a
third, while unsafe outputs still pass review.

Remaining failures:

* repeated-event/double-click semantics reduced to a per-click guarantee
* requested artifacts (a new regression test) replaced by an assumed one
* continuation either invents a project or reduces resumed work to editing a draft
* independent parallel-analysis requests, which produce no valid graph
* deficient final verification, where the output synthesizes instead of checking

**Follow-up decision:** none of the rounds was a net improvement over batch 7. Round 3 lowered
usability, halved auto-paste eligibility and raised approved critical fidelity failures. The
source generation and review instructions have therefore been restored to their batch-7 text.
The per-round instruction snapshots above preserve the experimental versions, and the evaluation
harness additions remain in place for future measurement.

Limitations: grading is by Copilot reading actual outputs. It is independent of the runtime
reviewer, but it is neither blinded nor human. The corpus is finite and synthetic. Request-ID
seeds are not paired across rounds whose call counts differ, so per-case differences include
sampling noise. Grades are not runtime validators and add no case-specific logic. The
instruction changes remain uncommitted, unreleased experiments. They should not be shipped
as a quality improvement on this evidence.
