---
description: Product north star for prompt generation, validation, evaluation, and quality changes.
applyTo: "**"
---

# Prompt quality north star

Every prompt-mode input, including a simple question, must become an advanced task graph with explicit dependencies, a bounded refinement loop, and verifiable completion criteria; do not remove this structure as a quality fix or apply this requirement to plain dictation.
Judge quality by faithful, useful expansion of the user's intent: add steps that help accomplish the request and checks that meaningfully improve the result, without inventing facts, unrelated tasks, or arbitrary restrictions.
For example, "Can you think of any other ways to improve prompt quality?" should become a graph that considers previously discussed approaches if available, proposes additional approaches with concrete explanations, checks relevance and duplication, and revises weak suggestions within a bounded loop; "excluding the current request itself" is an unjustified restriction.
Treat the user's live examples as regression cases alongside evaluation results: shorter output, valid graph syntax, reviewer approval, or successful pasting alone does not establish prompt quality.
