---
id: standards/index
title: Coding standards
doc-type: index
status: draft
owner: 'dpalfery'
last-reviewed: 2026-10-05
---

# Coding standards

How code is written in this repository, one directory per technology. A standard
is project-specific; the agents and skills that read it are not, which is why they
resolve it through the configuration registry rather than carrying their own.

## Declaring a technology

Add it to `ontology.technologies` in `.kyber-weave/kyber-weave.yml` and re-run
`kyber-weave docs init`. That one list creates the technology's folder, publishes its
`<name-coding-standard>` property in the Config Reg block of the repository root
`AGENTS.md`, and legalizes the `technology` value in the standard's frontmatter — so
the three cannot disagree.

A technology name is a slug: lowercase letters, digits and single hyphens.

The declared technologies are listed in that registry, which is regenerated on every
run. This file is not.
