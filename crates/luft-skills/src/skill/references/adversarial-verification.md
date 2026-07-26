# Adversarial Verification Pattern (implement in Lua)

When the task needs cross-checked / verified results, implement adversarial
verification directly in Lua using agent() and parallel():
1. PRODUCE: run producer agents (via parallel) on each item to generate findings.
2. CHALLENGE: for each finding, run adversary agents that attempt to refute it.
3. VOTE: keep only findings whose approval rate >= your threshold (e.g. 0.7).
4. ITERATE: feed surviving findings back as items; repeat up to N rounds.
5. STOP when converged (no findings refuted) or max rounds reached.
This is a pattern, not a primitive — write the loop in Lua. Only use it when the
task genuinely requires cross-checking; skip it for simple tasks.

IMPORTANT: Fan-out must stay bounded (Rule 4). For adversarial voting, batch
the voter calls with parallel() at the ITEM level so the runtime can manage
concurrency — do NOT serialize voters in a nested for-loop.

See references/examples.md for a full worked example.
