---
id: catalog
title: Component and owner catalog
doc-type: reference
status: draft
owner: 'dpalfery'
last-reviewed: 2026-10-05
---

# Component and owner catalog

This table is the **authoritative vocabulary** for the `component` and `owner`
frontmatter keys. A document naming a component with no row here fails
`KW-DOC-SPEC-004`. The check exists so that components cannot be invented one
document at a time until nobody can say how many there are.

One row per component that genuinely exists. Both components below are planned and their source roots don't exist yet. Don't add `source-root` to a document until the code lands.

| Component | Type | Source root | Overview | Detailed documentation | Owner | Last reviewed | Status |
|---|---|---|---|---|---|---|---|
| buzz-router | Service | `crates/buzz-router` (planned) | The per-machine daemon: relay listener, wake engine, adapters, local API, CLI, service install. Also the name of the system as a whole. | [System architecture](system/architecture.md), [v1 spec](specs/README.md) | dpalfery | 2026-10-05 | draft |
| router-core | Library | `crates/router-core` (planned) | Pure routing logic with no I/O: config types, author classification, mention, `@everyone` and stop parsing, `route()`, limit gates. | [v1 spec](specs/README.md) | dpalfery | 2026-10-05 | draft |

## How the columns are read

Only **Component** (index 1) and **Owner** (index 6) are parsed, counting the empty
cell produced by the leading pipe. The other columns are for human readers and may
be reworded freely. Moving either parsed column requires a matching
`ontology.catalog` override in `.kyber-weave/kyber-weave.yml`.
