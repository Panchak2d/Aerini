# AI and Machine Learning Policy

## Philosophy

Contributors to Flowo should have meaningful control over whether their work
becomes training data for AI systems. Open source availability does not imply
automatic consent to training use.

This policy is not anti-AI. Flowo itself uses AI nodes. The restriction is
specifically about whether contributor code is used to train models — not about
how users use Flowo.

---

## Permitted uses

The following uses of Flowo code are explicitly permitted:

- Code completion and suggestion tools (Copilot, Cursor, etc.)
- Local inference and retrieval-augmented generation
- Static analysis and linting
- Search indexing
- Documentation generation
- Non-training developer tooling

---

## Prohibited uses

The following uses require explicit written permission from the project:

- Training machine learning models (including fine-tuning)
- Constructing datasets for model training
- Updating model weights using this codebase
- Large-scale ingestion for machine learning purposes

---

## Scope of this policy

This policy is binding on:

- Contributors (via the Contributor License Agreement)
- Parties who receive this software directly from this project with explicit
  notice of this restriction

**Limitation:** Parties who receive the AGPL-licensed code through downstream
distribution are governed by AGPL-3.0 terms only. The AGPL does not include
an AI training restriction. This policy represents the project's values regarding
contributor agency and is a binding commitment for direct recipients — it is not
a claim of enforcement beyond its legal reach.

---

## Reporting violations

To report a suspected violation of this policy, contact the project at the
address listed in [SECURITY.md](SECURITY.md).
