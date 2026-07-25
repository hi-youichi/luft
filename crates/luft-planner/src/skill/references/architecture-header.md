# Architecture Header

Full notation legend, rules, and examples for the header comment introduced in the main skill body.

Diagram notation (indented arrows, NOT ASCII boxes):
- `==>`           sequential or fan-out flow between phases
- `<==`           fan-in: converge parallel branches back
- `--> [name]`    artifact produced by a step (hangs off the right side)
- `(for each X)`  decomposition dimension (X = module, file, finding, ...)
- `(retry <= N)`  bounded retry around a sub-chain
- `(degrade on fail)` optional: mark a sub-chain that should degrade on failure instead of abort
- `(parallel)`    branches run concurrently
- `(pipeline)`    branches run as staged pipeline
- Indentation (2 spaces per level) = nesting depth
- `|`             optional: links a phase to its artifact (visual aid only)

Rules:
- Two delimiter lines of 44 dashes wrapping the block.
- Goal: single English line stating what the workflow produces.
- Arch: read top-to-bottom; fan-out lines indent under their parent.
  Every `(for each X)` MUST eventually `<==` back. Show artifacts with `--> [name]`.
- Flow: single line showing global data flow as an artifact chain
  (e.g., discover -> subsystems[] -> modules[] -> report).
- This comment goes at the VERY TOP, before any schema locals or code.
- If the task is decomposed, the diagram MUST show the decomposition as
  a `(for each X)` fan-out with a matching `<==` fan-in.

Examples (every line carries the `-- ` comment prefix in real output):

Linear workflow:
--   discover ==> analyze ==> report
--     |              |
--     --> [targets]  --> [findings]

Parallel fan-out / fan-in:
--   plan ==> (parallel)
--     fetch --> [sources]
--     parse --> [docs]
--     index --> [chunks]
--   <== merge ==> report

Decomposed per-module with retry:
--   discover ==> (for each module)
--     analyze ==> change ==> verify --> [result]
--     (retry <= 2)        (degrade on fail)
--   <== report
