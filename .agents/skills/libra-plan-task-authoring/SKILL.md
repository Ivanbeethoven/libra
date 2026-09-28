---
name: libra-plan-task-authoring
description: Assess, select, or draft Libra development plans and task cards. Use for plan discovery, duplicate/conflict checks, dependency readiness, task-card decomposition, and plan authoring. Drafting is not authorization to implement; maintainers decide whether to accept proposals.
---

# Libra Plan and Task Authoring

Turn a clarified request into a decision-ready proposal or help select existing planned work. The agent may inspect and draft; the maintainer decides whether to accept the plan, card, scope, and priority. Drafting a plan or card does not authorize implementation.

## Read current guidance

Inspect the current checkout and read applicable sources:

- `AGENTS.md` for repository rules and command conventions.
- `docs/development/plan/plan-template.md` for current plan and task-card requirements.
- `docs/development/plan/plan-status.md` for active plans, card states, deferred work, and cross-plan dependencies.
- Relevant plans, especially `docs/development/plan/issues/<number>.md` when work maps to an Issue.
- Current source, tests, docs, and compatibility behavior to ground claims.

Treat these files as authoritative. Do not invent owners, dates, dependencies, commands, or acceptance criteria.

## Discover and assess existing work

1. Check `plan-status.md` and relevant plans for duplicates, conflicts, existing cards, and dependencies.
2. Ground recommendations in source, tests, docs, and compatibility behavior; separate evidence from assumptions.
3. For an existing card, report lifecycle, acceptance, prerequisite cards, `DEP-*` status, write-set conflicts, and exact start gates.

An Issue records a problem or requested outcome. In this repository, `docs/development/plan/issues/<number>.md` commonly maps to a GitHub Issue and serves as its executable plan. Dated plans may cover broader initiatives. A task card is independently actionable and has scope, dependencies, write set, deliverables, acceptance criteria, and verification.

In-plan dependencies reference concrete task IDs and form a directed acyclic graph; `A -> B` means A precedes B. Cross-plan, external-service, approval, and upstream-release dependencies must be registered as `DEP-*` before cards reference them. Check artifact, owner, evidence, availability criterion, and failure/timeout path. A card with no in-plan prerequisites may still be blocked by an external dependency, review gate, overlapping write set, or repository constraint. Do not call it ready until all applicable gates are satisfied.

## Draft plans and cards

Recommend one of these based on evidence:

- Reuse an existing card when scope and acceptance match.
- Propose one card for a single independently reviewable behavior axis.
- Propose a multi-card plan for distinct behavior axes, cross-surface changes, prerequisites, or release/compatibility coordination.
- Propose an audit or spike first when facts or feasibility are unknown; its deliverable should be evidence and a decision, not speculative implementation.

Use the current template and its G-* rules to split cards when a card combines independent behavior, cannot be reviewed or recovered as one unit, exceeds allowed scope, or has separable prerequisites/write sets. For each proposal include concrete IDs, deliverables, acceptance and verification, acyclic internal dependency edges, and registered cross-plan `DEP-*` dependencies with blocking conditions. Include tests, docs, compatibility, rollback, security, and release handling when relevant. Do not invent owners or promise dates; mark unknowns for maintainer decision.

Present the proposal as a draft for maintainer review. Do not begin implementation while required review, dependencies, or decisions are pending. Creating or editing repository plan files is a local write and should happen only when the user asks for those artifacts; it still does not authorize implementation of the cards. Before writing, inspect `libra status --short --branch`, preserve existing work, and avoid mixing unrelated changes. Respect the current branch unless the user or repository policy requires another branch operation; do not switch branches by default.

When advancing a card, update its plan and `plan-status.md` together as required by the current template. Keep lifecycle and acceptance distinct. Card completion does not automatically close an Issue, and plan completion does not prove every related Issue is resolved; summarize remaining work and leave remote Issue status to the user or maintainer.

## GitHub boundary and response

Use `gh` only when the user explicitly authorizes the specific remote action, such as viewing or creating a PR or a plan-mandated release. Check availability and authentication first. Do not post Issue comments, assign or label Issues, close Issues, merge, push, tag, or publish without explicit authorization for that operation.

Report evidence, proposed/existing identifiers, lifecycle and acceptance, dependencies, start gates, and maintainer decisions still needed. Briefly explain `DEP-*`, lifecycle, and acceptance when first used.
