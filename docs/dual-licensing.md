# Why Aerini is dual-licensed

Aerini ships under two licenses at once: AGPL-3.0 for open source use, and a separate commercial license for anyone who needs to get out from under AGPL's copyleft obligation. This page explains why that's the model, not just that it exists.

## The problem a single license can't solve

AGPL-3.0 is a strong copyleft license: if you run a modified version of `aerini-server` as a network-accessible service, you have to publish those modifications under AGPL-3.0 too. That's the right default for keeping Aerini and its ecosystem open. It's also a dealbreaker for some companies — building a closed-source product on top of Aerini, or shipping a modified version to clients without disclosing the changes, isn't compatible with AGPL on its own.

A commercial license removes that obligation for those cases, for one named product or service per license. That product can have any number of end customers; each additional product needs its own license. No source disclosure required, proprietary modifications permitted. See [FUNDING.md](https://github.com/Panchak2d/Aerini/blob/main/FUNDING.md) for what it's actually for and who needs one.

## Why that requires a CLA

Offering a commercial license only works if the maintainer actually has the right to relicense the whole codebase that way — including every contribution in it, not just the parts written by the maintainer. Without a Contributor License Agreement, an outside contribution could only ever go out under AGPL-3.0, since nothing would grant the maintainer the separate right to sublicense it commercially. That would fragment the codebase into "parts we can dual-license" and "parts we can't," which isn't workable.

The CLA solves this the same way most dual-licensed open source projects do: contributors grant the maintainer a non-exclusive license broad enough to support both tracks, while keeping full ownership and reuse rights over their own contribution. See [CLA-SUMMARY.md](https://github.com/Panchak2d/Aerini/blob/main/CLA-SUMMARY.md) for the plain-English breakdown, or [CLA.md](https://github.com/Panchak2d/Aerini/blob/main/CLA.md) Section 2 for the binding terms.

## What this doesn't change

- Contributions already released under AGPL-3.0 stay available under AGPL-3.0 — that grant is irrevocable.
- The AGPL-3.0 release isn't a "free tier" that could disappear if commercial licensing takes off. It's the default distribution channel, not a promotion.
- Signing the CLA doesn't transfer ownership, create an employment relationship, or entitle a contributor to commercial licensing revenue.

## See also

- [CLA-SUMMARY.md](https://github.com/Panchak2d/Aerini/blob/main/CLA-SUMMARY.md) — plain-English CLA summary and FAQ
- [CLA.md](https://github.com/Panchak2d/Aerini/blob/main/CLA.md) — the binding agreement
- [FUNDING.md](https://github.com/Panchak2d/Aerini/blob/main/FUNDING.md) — commercial licensing and supporting the project
