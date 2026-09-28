---
name: libra-issue-triage
description: Triage a Libra bug report, feature request, documentation issue, or usage question. Use to verify facts, check for duplicates, clarify scope-changing questions, and recommend a disposition. Read-only by default; do not plan task cards or implement changes unless the user asks for that next stage.
---

# Libra Issue Triage

Help the reporter and maintainer understand a reported problem and decide what information or decision is needed next. Keep verified facts, reporter statements, assumptions, and unknowns distinct. Maintainers decide acceptance, priority, labels, ownership, and disposition.

## Read current guidance

Before giving repository-specific conclusions, inspect applicable sources in the current checkout:

- `AGENTS.md` for repository rules and command conventions.
- `docs/contributing.md` for community contribution paths.
- `docs/development/plan/plan-status.md` and relevant plans/issues for existing work.
- Relevant source, tests, and user documentation for behavior claims.

Treat current files and code as authoritative. Older plan examples are historical evidence, not policy. Do not invent labels, owners, commands, dependencies, or acceptance criteria.

## Triage

1. Restate the observed problem or desired outcome.
2. Check expected and actual behavior, reproduction steps, environment, and user impact as applicable. Ask focused questions only when missing details change diagnosis or scope.
3. Classify provisionally as a bug, feature request, documentation issue, usage question, or unconfirmed report. Mark uncertain conclusions as hypotheses.
4. Search local plans, status records, source, tests, and docs for existing work before recommending a duplicate.
5. Recommend the next action: answer or gather evidence, link existing work, prepare a small direct change, or request plan/task-card authoring. Leave acceptance and priority to maintainers.

Maintain a compact Issue brief with verified facts, reporter-provided answers, assumptions, and unresolved questions separated. Capture goal and impact, current versus expected behavior, reproduction and environment when relevant, constraints, compatibility concerns, and success evidence. Do not fill gaps with guesses.

Ask only questions that can change diagnosis, scope, acceptance, priority, or dependencies. Group related questions, explain why they matter, incorporate answers, and do not ask again for information already supplied. Let the user choose how reporter Q&A proceeds:

- Continue in the current conversation; no GitHub access is needed.
- Draft a focused Issue comment for the user to post and bring back the reply.
- Use `gh` to read the specified Issue and post follow-up questions. This requires explicit authorization for that Issue and those interactions; check `gh` availability and authentication first. Keep comments within the agreed clarification purpose, summarize each response, and continue until the Issue brief is ready or a human decision is needed.

If the user has not selected a mode, continue locally and offer draft text; do not contact the reporter. Recheck the brief against repository sources after material answers. If an existing plan covers the report, identify it and whether the Issue changes its scope or acceptance. Stop discovery when problem, affected users, intended outcome, scope boundaries, acceptance evidence, and material dependencies are clear enough for planning. State maintainer-only choices as decisions needed.

## Recommend a disposition

Recommend one outcome and explain the evidence:

- Answer or gather more evidence.
- Link existing planned work and identify any scope or acceptance impact.
- Ask the plan/task-card authoring skill to assess one task card, a multi-card plan, or an audit/spike.
- Recommend an unsupported/duplicate disposition only when a maintainer directs that outcome; otherwise present it as a recommendation.

Do not author repository plan files as part of triage. Do not begin implementation from a triage recommendation.

## External actions and response

Triage is read-only unless explicitly authorized. Do not create, edit, label, assign, comment on, or close a GitHub Issue without explicit authorization for that specific action. Reading a remote Issue also requires the user to select that interaction mode and authorize the specified Issue. Never infer authorization for one remote action from authorization for another.

Report the current stage, evidence, unknowns, recommended next action, relevant identifiers or links, and which actions were local versus external. Keep the summary concise and leave maintainer decisions with maintainers.
