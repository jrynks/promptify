# Actual Promptify text-input quality grades

## Method

This is an end-to-end local-model test, not a fake generator, instruction preview,
or destination-task execution. Nine synthetic requests run through
`promptify-cli rewrite` under both `legacy` (default profiles) and `adaptive`
rendering. The CLI uses synthetic `FixedContext`, `NoHistory`, and `PrintInserter`;
no personal history, screen text, microphone, or native paste is accessed. The
running desktop app is left alone.

Model: **Qwen3.5 9B Q4_K_M**, installed ID `qwen3.5-9b-q4km`. Configuration:
temperature 0.3, top-k 40, top-p 0.9, min-p 0.05, worker-request-ID seed,
768 output tokens, maximum two internal rewrite repairs, 6,000 output characters,
the CLI's shared 120-second generation deadline. Every invocation loads its own
local worker. Job latency excludes preload; wall time includes it.

Command:

```sh
PROMPTIFY_EVAL_SHOW=1 /home/jrynx/.cache/promptify-target/debug/promptify-cli \
  rewrite "<synthetic request>" --process "<synthetic process>" \
  --rendering legacy --model qwen3.5-9b-q4km
```

Substitute `adaptive` for task-aware rendering; add `--url "<synthetic URL>"`
when supplied. The machine-readable results retain the exact arguments, actual
reports, returned prompts, and rejected repair text.

## Rubric

Each dimension receives a human-review score from **0 to 5**:

| Dimension | What is assessed |
| --- | --- |
| Intent fidelity | Preserves the current request, corrections, named facts, required actions and exclusions; does not do the destination task itself. |
| Dependency coherence | Steps do useful work; declared edges match actual prerequisites; parallel branches are independent and join before dependent work. |
| Correction and recheck | Specific failed checks lead to actual corrective work and repeat verification with bounded rounds and an honest exit. |
| Completion | Task-specific, observable checks cover the requested result rather than generic success, capability failure, or merely reaching the round limit. |
| Proportionality and fidelity | Appropriate workflow/detail for the request; no invented context, arbitrary counts, budgets, timing, assumptions presented as facts, or unnecessary task expansion. |

Anchors: **5** fully satisfies the dimension; **4** minor weakness; **3**
usable but noticeable omissions/ambiguity; **2** substantial weakness requiring
editing; **1** mostly ineffective or misleading; **0** absent or no usable
returned prompt. Overall `/10` is `2 × mean(the five scores)`, rounded to one
decimal. A failed job receives **0 in every dimension** because there is no
usable final output; rejected drafts are retained as diagnostic evidence, not
credited as successful outputs. Review-only `blocked` results may earn quality
credit: their prompts are returned but native insertion is correctly disabled.

These grades assess the prompt's instructions, not whether a destination AI
actually completes them. Structural acceptance alone earns no automatic semantic
score. No independent blinded reviewer, multiple-seed repeats, cloud judge, or
native-paste test is involved. The preceding implementation already used its
three allowed refinement rounds; this additional grading run does not restart
that budget or hide failures.


## Results

Scores: I = intent, G = graph, L = correction/recheck, C = completion, P = proportionality/fidelity. All component scores are `/5`.

| Case | Policy | Outcome | Repair status | Job / wall seconds | I | G | L | C | P | Overall /10 |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| explanation | legacy | inserted | repaired | 18.26 / 20.74 | 4 | 2 | 2 | 4 | 3 | **6.0** |
| coding | legacy | inserted | repaired | 24.39 / 25.99 | 4 | 4 | 4 | 4 | 4 | **8.0** |
| research | legacy | inserted | valid | 17.46 / 18.67 | 5 | 4 | 4 | 5 | 4 | **8.8** |
| writing | legacy | inserted | repaired | 47.29 / 49.01 | 4 | 3 | 3 | 2 | 1 | **5.2** |
| branches | legacy | inserted | repaired | 32.47 / 33.77 | 5 | 3 | 3 | 5 | 3 | **7.6** |
| correction | legacy | inserted | valid | 13.14 / 14.52 | 4 | 4 | 4 | 4 | 3 | **7.6** |
| inline | legacy | inserted | valid | 11.13 / 12.55 | 4 | 4 | 4 | 3 | 3 | **7.2** |
| creative | legacy | failed | exhausted | 43.27 / 44.73 | 0 | 0 | 0 | 0 | 0 | **0.0** |
| media | legacy | blocked | valid | 10.87 / 12.30 | 5 | 5 | 5 | 5 | 4 | **9.6** |
| explanation | adaptive | inserted | valid | 7.89 / 9.32 | 5 | 5 | 5 | 4 | 5 | **9.6** |
| coding | adaptive | blocked | repaired | 36.33 / 37.78 | 3 | 4 | 4 | 3 | 4 | **7.2** |
| research | adaptive | inserted | repaired | 38.15 / 39.37 | 5 | 3 | 3 | 4 | 4 | **7.6** |
| writing | adaptive | inserted | valid | 12.84 / 14.48 | 5 | 4 | 4 | 3 | 3 | **7.6** |
| branches | adaptive | inserted | repaired | 29.46 / 31.12 | 5 | 4 | 3 | 5 | 4 | **8.4** |
| correction | adaptive | inserted | valid | 11.71 / 13.26 | 5 | 4 | 4 | 3 | 4 | **8.0** |
| inline | adaptive | blocked | valid | 13.03 / 14.81 | 4 | 4 | 4 | 5 | 4 | **8.4** |
| creative | adaptive | inserted | valid | 11.16 / 12.76 | 5 | 4 | 4 | 5 | 4 | **8.8** |
| media | adaptive | blocked | repaired | 19.92 / 21.61 | 2 | 4 | 2 | 1 | 4 | **5.2** |

### Aggregate

- **legacy**: 8/9 usable returned prompts; 1 failure(s); 4 successfully repaired outputs; 1 review-only outcomes. Mean grade **6.67/10**, or **7.50/10** excluding unavailable outputs. Median job latency **18.26s**; median wall latency **20.74s**.
- **adaptive**: 9/9 usable returned prompts; 0 failure(s); 4 successfully repaired outputs; 3 review-only outcomes. Mean grade **7.87/10**, or **7.87/10** excluding unavailable outputs. Median job latency **13.03s**; median wall latency **14.81s**.
- **all**: 17/18 usable returned prompts; 1 failure(s); 8 successfully repaired outputs; 4 review-only outcomes. Mean grade **7.27/10**, or **7.69/10** excluding unavailable outputs. Median job latency **17.86s**; median wall latency **19.70s**.

### Interpretation and limits

The grade is **mixed, not a blanket quality pass**. The default coding fix is useful; the adaptive simple explanation is compact and coherent. Correction requests drop the cancelled email. Both media outputs remain review-only, and the default media workflow genuinely generates and inspects an image. However, the adaptive media output substitutes prompt drafting for image creation; the default writing output invents personal context and adds placeholders after checking; default independent analyses are unnecessarily serialized. The default creative request exhausts repairs and returns no usable prompt. Structurally accepted loops sometimes point to explanation or summary steps rather than actual verification. These failures remain visible in the grades and raw outputs.

No additional refinement rounds were used: the preceding implementation already consumed the requested maximum of three. Repairs stay inside existing limits, and failed drafts are not automatically pasted. Single runs on a shared machine do not establish latency distributions, robustness across random seeds, semantic correctness, actual recipient execution, or native delivery reliability. `inserted` in these CLI reports means **PrintInserter accepted the text**, not insertion into a real app.

## Requests, actual generated prompts, and grading evidence

The full machine-readable evidence is [graph-quality-graded-results.json](graph-quality-graded-results.json). Every returned output below is verbatim.

### explanation / legacy — 6.0/10

**Text request:** Explain why the sky looks blue.

**Evidence:** Keeps the blue-sky goal, but Step 2 explains blue versus violet rather than performing an explicit check. The loop labels it a verifier without making it one; accuracy criteria are relevant, while Context and Constraints add unnecessary scaffolding.

```text
I need to explain why the sky looks blue.

Context: This is a general science explanation request with no specific audience, location, or prior knowledge level stated.

Constraints: Keep the explanation clear and accessible without inventing specific dates, names, or complex jargon unless necessary for accuracy.

Task graph:
Step 1: Explain the physical mechanism of Rayleigh scattering, detailing how shorter wavelengths (blue/violet) scatter more effectively than longer wavelengths when sunlight interacts with the atmosphere.
Step 2 (after 1): Summarize why the human eye perceives the sky as blue rather than violet, considering both the scattering spectrum and human visual sensitivity.
Loop: if Step 2 fails the stated accuracy checks regarding human visual perception, return to Step 1 to correct the inaccurate content; then recheck Step 2 (max 2 rounds).
Done when: The explanation accurately describes Rayleigh scattering, correctly identifies the role of human eye sensitivity in perceiving blue over violet, and the verification loop confirms these points without factual errors.
```

### coding / legacy — 8.0/10

**Text request:** The login button sends two requests when clicked twice. Fix that and add a regression test, then run the relevant tests. Do not change the API.

**Evidence:** Preserves the double-click bug, regression test, test execution and unchanged API. Sequential analysis/fix/test/run edges are coherent. The loop revisits implementation, but does not explicitly update a faulty regression test. 'Regardless of click speed' and 'one successful request' broaden the supplied behavior.

```text
I need to fix a bug where the login button triggers two requests when clicked twice, add a regression test for this fix, and run the relevant tests without altering the API.

Step 1: Analyze the login button's current event handling and network traffic to identify the root cause of the duplicate request generation upon double-clicking.
Step 2 (after 1): Implement a fix that prevents the second request while ensuring the first request still executes correctly, strictly adhering to the constraint that the API definition and contract must not be changed.
Step 3 (after 2): Write a regression test specifically designed to verify that clicking the login button twice results in exactly one successful request and no duplicate network calls.
Step 4 (after 3): Execute the relevant test suite to confirm the regression test passes and no existing tests are broken by the changes.
Loop: if Step 4 fails the stated accuracy checks (regression test passes, no duplicate requests, API unchanged), return to Step 2 to correct the implementation logic; then recheck Step 4 (max 2 rounds).
Done when: The login button triggers exactly one request regardless of click speed, the new regression test exists and passes, the full relevant test suite passes, and the API contract remains identical to the original.
```

### research / legacy — 8.8/10

**Text request:** Compare three task tracking tools for our volunteer team. Keep costs low, cite current sources, and flag uncertain prices.

**Evidence:** Preserves three tools, volunteers, low cost, current citations and uncertain prices. Pricing research feeds comparison and checking. The bounded loop explicitly re-researches pricing and re-evaluates the recommendation. Six steps plus risk analysis/recommendation are somewhat more than the requested comparison.

```text
I need to compare three task tracking tools for our volunteer team, prioritizing low cost, citing current sources, and flagging any uncertain prices.

Before you start, ask only necessary questions about our team size, specific volunteer management needs, or required features if the answer depends on them.

Step 1: Identify three distinct task tracking tools suitable for volunteer teams, focusing on those with lower total cost structures.
Step 2 (after 1): Research and list the current pricing for each tool, citing specific sources and dates; flag any prices that are uncertain, estimated, or require contact for a quote.
Step 3 (after 2): Analyze the trade-offs between the three options, weighing cost against features relevant to volunteer management.
Step 4 (after 3): Evaluate potential risks or downsides for each option based on the analysis.
Step 5 (after 4): Recommend the best option based on the low-cost constraint and feature fit, stating assumptions if details are missing.
Step 6 (after 5): Verify that all cited prices are current, all uncertainties are flagged, and the recommendation logically follows the cost analysis.
Loop: if Step 6 fails the verification checks (missing citations, unflagged uncertainties, or illogical recommendation), return to Step 2 to re-research pricing and Step 5 to re-evaluate the recommendation; then recheck Step 6 (max 2 rounds). Stop when checks pass; at the limit report unmet criteria.
Done when: three tools are compared with current cited prices, uncertainties are flagged, risks are analyzed, and a single recommendation is made that adheres to the low-cost constraint.

Format: A comparison table for the three tools (Name, Current Price, Source/Citation, Uncertainty Flag), followed by a numbered list of risks, and a final paragraph with the recommendation.
```

### writing / legacy — 5.2/10

**Text request:** Draft a polite email asking for the meeting notes. Do not apologize or invent a deadline.

**Evidence:** Keeps polite notes request, no apologies and no invented deadline. Invents that the user has not drafted yet and lacks recipient/date details. Step 3 adds placeholders after Step 2 verification despite an earlier prohibition. Done when treats reaching the limit and reporting limitations as completion.

```text
I need to draft a polite email requesting meeting notes without apologizing or inventing a deadline.

Context:
- This is for a professional setting where I need to follow up on a meeting that has already occurred.
- I do not have the specific meeting date, participants, or the name of the recipient.
- I have not tried drafting this yet.

Constraints:
- The tone must be polite and professional.
- Do not include any apologetic language (e.g., "sorry for the late request").
- Do not invent or suggest a specific date or deadline for when the notes are due.
- Do not use placeholders like "[Name]" or "[Date]" for facts I haven't provided; instead, write the email so the user can fill those in, or state the assumption clearly if a specific phrasing is required.

Task graph:
Step 1: Draft the email body focusing on a clear, direct request for the meeting notes while maintaining a professional and courteous tone.
Step 2 (after 1): Review the draft as a skeptical editor to ensure no apologetic language is present and no artificial deadlines are invented.
Step 3 (after 2): Finalize the email by adding a subject line and ensuring the placeholders for specific meeting details (like date, time, or attendee names) are clearly marked for the user to fill in.
Loop: if Step 2 fails the stated accuracy checks (presence of apology or invented deadline), return to Step 1 to correct the tone and content; then recheck Step 2 (max 2 rounds).

Output format:
- Provide the final email draft in a code block or clearly separated section.
- Include a brief note below the draft listing the assumptions made about missing details (e.g., "Assumed the user will insert the specific meeting date here").

Done when:
- The email draft is complete with a subject line and body.
- The draft contains zero instances of apologetic language.
- The draft contains zero invented deadlines or specific dates for submission.
- The loop has either passed the accuracy checks or reported the specific limitations after reaching the maximum rounds.
```

### branches / legacy — 7.6/10

**Text request:** Compare the supplied support tickets and interview notes independently, then combine their findings into an onboarding improvement plan. Preserve disagreements, cite the supplied evidence, and do not invent customer quotes or dates.

**Evidence:** Preserves both evidence sources, disagreements, onboarding plan and no invented quotes/dates. Independent interview analysis unnecessarily depends on ticket analysis; synthesis names both sources but only lists after 2. The correction targets the draft, not missing analysis/disagreements, and formatting adds another step after verification. Strong source-specific completion checks.

```text
I need to compare the supplied support tickets and interview notes independently, then combine their findings into an onboarding improvement plan while preserving disagreements and citing evidence without inventing quotes or dates.

Context:
- I have access to specific support tickets and interview notes regarding the onboarding process.
- I do not have access to external data, customer names, specific dates, or internal metrics not present in the supplied documents.

Constraints:
- Do not invent customer quotes, dates, or specific customer names.
- Do not hallucinate metrics or data not found in the supplied tickets and notes.
- Explicitly preserve any disagreements between the support ticket data and the interview notes.
- Cite the specific evidence (ticket ID, note reference, or excerpt) for every finding.

Task structure:
Step 1: Independently analyze the supplied support tickets to extract recurring pain points, failure modes, and specific user frustrations related to onboarding.
Step 2 (after 1): Independently analyze the supplied interview notes to extract qualitative insights, user sentiments, and identified gaps in the onboarding experience.
Step 3 (after 2): Synthesize the findings from Step 1 and Step 2 into a unified onboarding improvement plan, explicitly noting where the support data and interview data agree or disagree.
Step 4 (after 3): Draft the final improvement plan, ensuring every recommendation is backed by a specific citation from the supplied tickets or notes.
Step 5 (after 4): Review the draft as a skeptical auditor to verify that no invented quotes, dates, or metrics have been introduced and that all disagreements are preserved.
Step 6 (after 5): Format the output as a structured report with clear sections for Findings, Disagreements, and the Improvement Plan.

Loop: if Step 5 fails the stated accuracy checks (specifically finding invented quotes, dates, or missing citations for recommendations), return to Step 4 to correct the inaccurate content; then recheck Step 5 (max 3 rounds).

Done when: The final output contains a structured improvement plan where every claim is cited to a specific ticket or note, all disagreements between sources are explicitly listed, and no external facts (dates, names, quotes) have been invented.
```

### correction / legacy — 7.6/10

**Text request:** Write an email about the delay, no wait translate the supplied paragraph into French. Keep the names and numbers unchanged; do not summarize it.

**Evidence:** Correctly drops the retracted email task and requests French translation without summarizing, retaining names/numbers. Step 1 oddly asks to verify that no names/numbers are present instead of identify what must be preserved. Translation/review edges and bounded correction/recheck are coherent, but the four-step workflow is longer than needed.

```text
Translate the provided text into French, keeping all names and numbers exactly as they appear, without summarizing.

Before you start, ask only necessary questions about the specific text to translate if it is not clearly provided in the context.

Step 1: Identify the source text to be translated and verify that no names or numbers are present in the prompt that need preservation.
Step 2 (after 1): Translate the text into French, ensuring strict adherence to the constraint of leaving all names and numbers unchanged.
Step 3 (after 2): Review the translation to confirm that no content was summarized and that the original names and numbers match the source exactly.
Step 4 (after 3): Check that the French grammar and syntax are correct while maintaining the original structure and specific terms.
Loop: if Step 3 fails the accuracy check regarding names, numbers, or summarization, return to Step 2 to correct the translation; then recheck Step 3 (max 2 rounds). Stop when checks pass; at the limit report unmet criteria.
Done when: the output is a complete French translation with no summarization and all names and numbers identical to the source.

Format: Provide only the translated text.
```

### inline / legacy — 7.2/10

**Text request:** Fix the flaky API tests and run the suite until it passes. Do not skip failing tests or invent timeouts.

**Evidence:** Single-line graph preserves flaky API tests, suite execution, no skips and no artificial timeouts. Diagnosis feeds fixes and suite checks; bounded loop corrects failures. Suggesting retries before diagnosis risks masking flakiness, and 'passes consistently' has no stated evidence standard. Exhaustion reporting is absent.

```text
I need to identify and fix the flaky API tests in my project, run the full suite until it passes, and ensure no tests are skipped or artificially timed out. Step 1: run the full test suite to capture the current flaky failures and log the specific errors; Step 2 (after 1): analyze the logs to determine if failures are due to race conditions, environment instability, or transient network issues; Step 3 (after 2): implement fixes for the identified root causes and add necessary retry logic or synchronization without introducing artificial timeouts; Step 4 (after 3): re-run the complete suite including the previously failing tests to verify stability; Loop: if Step 4 fails the stability checks or reveals new failures, return to Step 3 to correct the remaining issues; then recheck Step 4 (max 3 rounds); Done when: the full suite passes consistently without skipping any tests and the original flaky failures no longer occur.
```

### creative / legacy — 0.0/10

**Text request:** Write a gentle bedtime story about a lighthouse and a lost bird. No scary scenes, and do not impose a word count.

**Evidence:** No usable prompt: repairs exhausted because the loop used 'if Step 3 identifies' rather than the required 'fails' grammar. The rejected draft is retained but receives no quality credit; nothing was returned as insertable or copyable output.

**No usable final prompt returned.** The raw JSON retains the failure report and rejected repair.

### media / legacy — 9.6/10

**Text request:** Create an image of a fox in a forest at night with glowing mushrooms. No text in the image.

**Evidence:** Keeps fox, nighttime forest, glowing mushrooms and no text. Creation feeds explicit visual inspection; failures refine and regenerate, then recheck with two rounds and honest limits. Completion names every requested visual property. Selecting Midjourney from the synthetic destination assumes tool availability, but review-only delivery correctly blocks paste.

```text
Generate an image of a fox in a forest at night with glowing mushrooms, ensuring no text appears in the final output.; Step 1: Use the Midjourney tool in Chrome to generate an image based on the subject (fox), setting (forest at night), and details (glowing mushrooms) while explicitly excluding text.; Step 2 (after 1): Inspect the generated image to verify the presence of the fox, the forest night setting, glowing mushrooms, and the complete absence of any text.; Loop: if Step 2 fails the subject, setting, detail, or text-exclusion checks, return to Step 1 to refine the prompt and regenerate; then recheck Step 2 (max 2 rounds).; Done when: the image clearly depicts a fox in a forest at night with glowing mushrooms and contains absolutely no text. Stop on passing checks; report tool limitations or unverified criteria rather than claiming success at the limit.
```

### explanation / adaptive — 9.6/10

**Text request:** Explain why the sky looks blue.

**Evidence:** Compact two-step work/verification graph preserves the science question without answering it locally. Accuracy/consistency failures correct mechanism/example and recheck with two rounds and uncertainty reporting. Completion is topic-specific, although technical accuracy still requires recipient judgment.

```text
Explain why the sky looks blue.
Step 1: Explain the physical mechanism causing the sky's blue appearance, defining necessary terms and using a practical example.
Step 2 (after 1): Check the explanation for technical accuracy and consistency between the mechanism and example.
Loop: if Step 2 fails the accuracy or consistency checks, return to Step 1 to correct the mechanism and example; then recheck Step 2 (max 2 rounds).
Done when: the scattering mechanism is explained accurately, terms are defined, and the example matches the mechanism. Stop on passing checks; at the limit report uncertainty.
```

### coding / adaptive — 7.2/10

**Text request:** The login button sends two requests when clicked twice. Fix that and add a regression test, then run the relevant tests. Do not change the API.

**Evidence:** Preserves API immutability and tests, but Step 2's 'one request per click' conflicts with suppressing the second rapid click; Step 3 is clearer. Dependencies are ordered and failed tests trigger bounded correction/recheck. Done when tests pass without explicitly requiring the double-click scenario, weakening fidelity. Unconfirmed code input correctly returns review-only.

```text
Fix the double-request bug in the login button and add a regression test, then run the relevant tests without changing the API.

Step 1: Analyze the login button click handler to identify the logic causing two API requests when the button is clicked twice.
Step 2 (after 1): Implement a fix to ensure only one request is sent per click, strictly without altering the API endpoints or their payloads.
Step 3 (after 2): Write a regression test specifically designed to verify that the login button sends exactly one request upon a single click and does not send extra requests on subsequent rapid clicks.
Step 4 (after 3): Run the relevant existing tests and the new regression test to confirm the fix works and no regressions were introduced.
Loop: if Step 4 fails any tests or Step 3 does not accurately verify the single-request behavior, return to Step 2 to correct the implementation; then recheck Step 4 (max 2 rounds).
Done when: Step 4 passes all relevant tests including the new regression test, and the API endpoints remain unchanged.
```

### research / adaptive — 7.6/10

**Text request:** Compare three task tracking tools for our volunteer team. Keep costs low, cite current sources, and flag uncertain prices.

**Evidence:** Preserves three tools, low cost, current citations and price uncertainty. Step 3 summarizes rather than explicitly verifies, yet serves as the loop checker; the later comparison in Step 4 is not rechecked. Completion covers prices/sources but 'all prices are current' conflicts with permitted uncertain prices. Four steps are proportionate.

```text
Compare three task tracking tools for a volunteer team, prioritizing low cost, citing current sources and flagging any uncertain prices.

Step 1: Identify three distinct task tracking tools suitable for volunteer teams, ensuring each option is evaluated against low-cost criteria.
Step 2 (after 1): Research current pricing, features, and availability for each of the three tools, citing specific sources.
Step 3 (after 2): Summarize the findings for each tool, explicitly noting any price uncertainties or missing information.
Step 4 (after 3): Present a concise comparison highlighting the most cost-effective option while maintaining transparency about uncertainties.
Loop: if Step 3 fails to clearly flag uncertain prices or lacks current source citations, return to Step 2 to re-research and correct the inaccurate content; then recheck Step 3 (max 2 rounds).
Done when: The comparison includes three tools, all prices are current and sourced, uncertainties are explicitly flagged, and the output is concise.
```

### writing / adaptive — 7.6/10

**Text request:** Draft a polite email asking for the meeting notes. Do not apologize or invent a deadline.

**Evidence:** Preserves polite request, no apology and no invented deadline, with no invented personal context. Four steps defer missing-context clarification until after drafting. Final verification and correction are connected, but Done when allows merely asking a question instead of delivering the requested draft.

```text
Draft a polite email asking for the meeting notes without apologizing or inventing a deadline.
Step 1: Draft a polite, direct request for the meeting notes ensuring no apologies are included and no specific deadline is invented.
Step 2 (after 1): Check if the meeting date, participants, or context are missing; if so, ask at most one clarifying question about the most critical missing detail.
Step 3 (after 2): If clarification is provided, refine the draft to include the necessary context while maintaining the no-apology and no-deadline constraints.
Step 4 (after 3): Verify the final draft is polite, contains no apologies, no invented deadlines, and includes all provided context.
Loop: if Step 4 fails the politeness, constraint (no apology/deadline), or context-completeness checks, return to Step 1 to revise the draft; then recheck Step 4 (max 2 rounds).
Done when: the email draft is polite, free of apologies and invented deadlines, and either includes all necessary context or clearly requests it via one question. Stop on passing checks; at the limit report unmet criteria.
```

### branches / adaptive — 8.4/10

**Text request:** Compare the supplied support tickets and interview notes independently, then combine their findings into an onboarding improvement plan. Preserve disagreements, cite the supplied evidence, and do not invent customer quotes or dates.

**Evidence:** Retains independent analysis of both sources, cross-reference, disagreements, evidence citations and no invented quotes/dates. The sequential chain is coherent; parallelism is optional, though both analyses share one step. Verification checks quotes/dates and claims but not explicitly preservation of disagreements; the loop's 'accuracy checks' correction is generic.

```text
Compare independently the supplied support tickets and interview notes, then synthesize their findings into an onboarding improvement plan while preserving disagreements, citing supplied evidence, and avoiding invented customer quotes or dates.

Step 1: Read and extract all factual claims, dates, and customer quotes from the supplied support tickets and interview notes.
Step 2 (after 1): Independently analyze the support tickets to identify onboarding friction points, then independently analyze the interview notes to identify onboarding friction points.
Step 3 (after 2): Cross-reference findings between the two sources to identify areas of agreement and areas of disagreement.
Step 4 (after 3): Draft the onboarding improvement plan, integrating agreed findings and explicitly listing disagreements with their respective evidence citations.
Step 5 (after 4): Verify that no customer quotes or dates were invented and that all claims are cited to supplied evidence.
Loop: if Step 5 fails the stated accuracy checks, return to Step 4 to correct the inaccurate content; then recheck Step 5 (max 2 rounds).
Done when: The plan integrates agreed findings, explicitly lists disagreements with evidence citations, contains no invented quotes or dates, and all claims are sourced to supplied evidence.
```

### correction / adaptive — 8.0/10

**Text request:** Write an email about the delay, no wait translate the supplied paragraph into French. Keep the names and numbers unchanged; do not summarize it.

**Evidence:** Correctly replaces the cancelled email with French translation and retains names/numbers and no summarizing. Missing input is requested instead of invented; translation feeds fidelity checking and correction/recheck. The final sentence treats obtaining missing text as the goal, weakening translation completion; round-limit reporting is absent.

```text
Translate the supplied paragraph into French, keeping names and numbers unchanged and without summarizing.
Step 1: Identify the paragraph to translate and verify if the specific text content is provided; if the text is missing, ask the user to supply it.
Step 2 (after 1): Translate the paragraph into French, ensuring all proper names and numerical values remain exactly as in the original and no content is summarized or omitted.
Step 3 (after 2): Review the translation to confirm that the meaning is preserved, names are unchanged, numbers are unchanged, and the text is not summarized.
Loop: if Step 3 fails the fidelity checks (names, numbers, no summary), return to Step 2 to correct the translation; then recheck Step 3 (max 2 rounds).
Done when: the French output is a direct translation of the provided paragraph with all names and numbers intact, no summarization, and meaning preserved. Stop when checks pass; if the text was missing, the goal is to have obtained it.
```

### inline / adaptive — 8.4/10

**Text request:** Fix the flaky API tests and run the suite until it passes. Do not skip failing tests or invent timeouts.

**Evidence:** Preserves no skipped failures/timeouts and full-suite verification in one line. Independent configuration preparation joins fixes before execution. 'Propose or implement' permits stopping short of the requested fix; loop permits alternate correction targets and adjusted parameters without clear limits. Completion and exhaustion reporting are concrete; unconfirmed terminal input correctly blocks paste.

```text
Fix the flaky API tests and run the suite until it passes without skipping failing tests or inventing timeouts.; Step 1: Identify the specific flaky API tests and their failure patterns based on the provided context.; Step 2 (after 1): Propose or implement fixes for the identified flaky tests without inventing timeouts or skipping failures.; Step 3 (after 1; parallel with 2): Prepare the test suite configuration to run the full suite including the fixed tests.; Step 4 (after 2, 3): Execute the full test suite and monitor results for passing status.; Step 5 (after 4): Verify that all tests pass and no failures were skipped or artificially timed out.; Loop: if Step 5 fails the verification that all tests pass without skipped failures or invented timeouts, return to Step 2 to refine fixes or Step 4 to re-run with adjusted parameters; then recheck Step 5 (max 2 rounds).; Done when: the full API test suite passes with no skipped failures, no invented timeouts, and all fixes verified; if the limit is reached, report the remaining unmet criteria and limitations.
```

### creative / adaptive — 8.8/10

**Text request:** Write a gentle bedtime story about a lighthouse and a lost bird. No scary scenes, and do not impose a word count.

**Evidence:** Keeps lighthouse, lost bird, gentle bedtime mood, no scary scenes and no imposed word count. Draft feeds content verification and tone/pacing checks. The loop corrects content then rechecks Step 2 but omits the later tone check. Task-specific completion and honest exhaustion are present; three steps are proportionate.

```text
Write a gentle bedtime story about a lighthouse and a lost bird, ensuring no scary scenes appear and avoiding any imposed word count.
Step 1: Draft a gentle bedtime story featuring a lighthouse and a lost bird, maintaining a soothing tone and avoiding any frightening imagery.
Step 2 (after 1): Verify the story contains no scary scenes and that the lighthouse and lost bird are central to the narrative.
Step 3 (after 2): Check the story's pacing and tone to ensure it fits a gentle bedtime atmosphere.
Loop: if Step 2 fails the check for scary scenes or missing central characters, return to Step 1 to revise the content; then recheck Step 2 (max 2 rounds).
Done when: the story is gentle, features the lighthouse and lost bird, contains no scary scenes, and has no imposed word count. Stop on passing checks; at the limit report unresolved issues.
```

### media / adaptive — 5.2/10

**Text request:** Create an image of a fox in a forest at night with glowing mushrooms. No text in the image.

**Evidence:** Retains every visual detail and no text, but replaces creating an image with drafting an image prompt. Dependencies and a bounded text correction/recheck are present; no image-generation or image-inspection step exists. Done when verifies only the prompt, not the requested image. Review-only blocking is correct but does not repair the intent substitution.

```text
Generate an image of a fox in a forest at night with glowing mushrooms containing no text; Step 1: Draft the image prompt describing a fox in a forest at night with glowing mushrooms and explicitly excluding text; Step 2 (after 1): Verify the prompt meets the requirements of a fox, forest, night, glowing mushrooms, and no text; Loop: if Step 2 fails the stated requirements, return to Step 1 to correct the mismatches; then recheck Step 2 (max 2 rounds); Done when: the final image prompt describes a fox in a forest at night with glowing mushrooms and explicitly excludes text.
```i
