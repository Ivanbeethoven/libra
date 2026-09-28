---
name: libra-contribution-execution
description: Implement and deliver a Libra task card selected by the user with implementation authorization. Use for code or documentation changes, focused verification, plan status updates, and PR preparation. Do not use for issue triage or deciding what work should be planned.
---

# Libra Contribution Execution

Implement the selected, authorized task card and prepare a reviewable contribution. Follow current repository policy and report actual results. If scope expands, dependencies change, or write sets overlap, stop the affected work and request a maintainer decision before proceeding.

## Confirm the authorized task

Before edits, inspect the current checkout and read:

- `AGENTS.md` for repository rules, validation, and version-control workflow.
- The selected task card and its plan/status entries.
- Relevant source, tests, docs, and compatibility behavior.

Refresh anchors against the current checkout. Confirm the user selected the card and authorized implementation. Preserve existing work and identify unrelated changes before editing. Follow the card's scope and write set. If the task is not clearly selected or authorized, return to plan/task authoring rather than inferring permission from a draft.

## Branch and worktree context

This skill does not require a particular branch, branch name, or base, and does not require switching before implementation. Respect the branch the contributor is already using unless the contributor, task card, or current repository policy specifies a different branch operation.

Inspect `libra status --short --branch` before edits. If the worktree is dirty, identify unrelated changes and avoid mixing or overwriting them. If branch selection matters but is unspecified, explain the context and ask before creating or switching branches. When a branch operation is explicitly required, use Libra commands and verify the result; do not use raw Git for repository state in this Libra checkout. Read-only investigation does not require a branch change.

## Implement and verify

1. Follow the card's scope and write set. If scope expands, dependencies change, or write sets overlap, update the plan or resolve sequencing with the maintainer first.
2. Add or update focused tests and user documentation as required by the card and repository rules.
3. Run the exact verification required by the card and current repository guidance. Follow current ER-13/ER-14 rules; do not copy obsolete commands from old plans.
4. Report actual results. Do not mark a card complete while acceptance, release, or remote gates remain pending.
5. Update the card and `plan-status.md` in the same change when lifecycle, acceptance, dependency, or deferred status changes.

Use the project's Libra command workflow for repository operations. Follow current DCO/signing and commit/push instructions in `AGENTS.md`. Do not commit, push, merge, tag, or publish unless the user explicitly authorized that operation.

## PR and GitHub interactions

Prepare a concise PR description with intent, related Issue(s), changes, tests actually run, and reproduction or sample output for user-visible behavior. Use `gh` only when the user explicitly authorizes the specific live GitHub action, such as viewing or creating a PR. Check `gh` availability and authentication first. Do not post Issue comments outside authorized Q&A, assign or label Issues, close Issues, merge, push, tag, or publish a release without explicit authorization for that operation.

After review feedback, map requested changes to the relevant card and acceptance criteria. Keep local plan/card status separate from GitHub Issue/PR status and report unsynchronized state. Explain blockers and the decision needed; do not silently broaden the task.

## Response

Summarize the selected card, files changed, verification actually run and results, remaining acceptance or release gates, and any local versus external status. Do not claim unrun checks passed or mark incomplete work complete.
