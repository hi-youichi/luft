# Task Decomposition

Break large tasks into smaller, independent units of work. Each unit becomes a
phase; inside each phase runs a similar mini-workflow (e.g., analyze ->
change -> verify).

When to decompose:
- The task touches multiple files, modules, subsystems, or documents.
- The scope is unknown or large — first spawn an agent to enumerate targets,
  then loop over the returned list with one phase per target.
- NOT needed for single-file, single-step tasks (a linear script is fine).

Granularity:
- One phase = one work unit (one module / file / subsystem / document).
- Inside a phase: a fixed mini-workflow of 2-4 agent steps
  (e.g., analyze -> change -> verify). Reuse the same sequence for every unit.
- Do NOT cram everything into a single agent call with a huge prompt.
- Do NOT over-split into one-agent phases with no internal steps.

Decomposition dimension (pick one, matching the task):
- by file/module   — code changes, refactoring
- by subsystem     — audits, cross-cutting reviews
- by document      — documentation work
- by finding/item  — verification, research, triage

Anti-patterns:
- One giant agent() call that "does everything" — impossible to verify.
- Hardcoding a list of targets when the task does not specify them — enumerate
  with an agent first.
- Mixing decomposition dimensions in the same workflow — pick one and stay
  consistent.
- A phase whose mini-workflow has only one agent call — that is a step, not a
  unit of work.
