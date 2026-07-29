-- opencode-conformance-audit.lua
--
-- Per-endpoint conformance audit: compare OpenCode contract against Loom
-- implementation. Three-phase pipeline with adversarial verification.
--
-- Phase 1: Audit     — 61 agents, read contract + Loom code, find deviations
-- Phase 2: Adversary  — 183 agents (3 per endpoint: schema / behavior / boundary)
-- Phase 3: Doc Write  — 61 doc agents + 1 index agent
--
-- Total: 306 agents. No budget limit.

meta = {
  reasoning = "Conformance audit: 61 endpoints × (1 audit + 3 adversary + 1 doc) + 1 index = 306 agents. Each endpoint gets schema/behavior/boundary adversarial review.",
  phases = {
    { label = "audit",     description = "61 audit agents (parallel, cap=4)",  dynamic = true },
    { label = "adversary", description = "183 adversary agents (parallel, cap=4)", dynamic = true },
    { label = "doc-write", description = "61 doc-write + 1 index agent",         dynamic = true },
  },
}

----------------------------------------------------------------------
-- Endpoint list (61 endpoints — same as opencode-endpoint-docs.lua)
----------------------------------------------------------------------

local ENDPOINTS = {
  { "GET", "/api/agent",                                   "agent.list",            "agent.ts" },
  { "GET", "/api/command",                                 "command.list",          "command.ts" },
  { "PATCH",  "/api/credential/:credentialID",             "credential.update",     "credential.ts" },
  { "DELETE", "/api/credential/:credentialID",             "credential.remove",     "credential.ts" },
  { "GET", "/api/event",                                   "event.subscribe",       "event.ts" },
  { "GET", "/api/fs/read/*",                               "fs.read",               "fs.ts" },
  { "GET", "/api/fs/list",                                 "fs.list",               "fs.ts" },
  { "GET", "/api/fs/find",                                 "fs.find",               "fs.ts" },
  { "GET", "/api/health",                                  "health.get",            "health.ts" },
  { "GET",    "/api/integration",                          "integration.list",            "integration.ts" },
  { "GET",    "/api/integration/:integrationID",           "integration.get",             "integration.ts" },
  { "POST",   "/api/integration/:integrationID/connect/key",  "integration.connect.key",  "integration.ts" },
  { "POST",   "/api/integration/:integrationID/connect/oauth", "integration.connect.oauth", "integration.ts" },
  { "GET",    "/api/integration/attempt/:attemptID",       "integration.attempt.status",  "integration.ts" },
  { "POST",   "/api/integration/attempt/:attemptID/complete", "integration.attempt.complete", "integration.ts" },
  { "DELETE", "/api/integration/attempt/:attemptID",       "integration.attempt.cancel",  "integration.ts" },
  { "GET", "/api/location",                                "location.get",          "location.ts" },
  { "GET", "/api/session/:sessionID/message",              "session.messages",      "message.ts" },
  { "GET", "/api/model",                                   "model.list",            "model.ts" },
  { "GET",    "/api/permission/request",                   "permission.request.list",   "permission.ts" },
  { "GET",    "/api/permission/saved",                     "permission.saved.list",     "permission.ts" },
  { "DELETE", "/api/permission/saved/:id",                 "permission.saved.remove",   "permission.ts" },
  { "POST",   "/api/session/:sessionID/permission",        "session.permission.create", "permission.ts" },
  { "GET",    "/api/session/:sessionID/permission",        "session.permission.list",   "permission.ts" },
  { "GET",    "/api/session/:sessionID/permission/:requestID", "session.permission.get",  "permission.ts" },
  { "POST",   "/api/session/:sessionID/permission/:requestID/reply", "session.permission.reply", "permission.ts" },
  { "POST",   "/experimental/project/:projectID/copy",            "projectCopy.create",  "project-copy.ts" },
  { "DELETE", "/experimental/project/:projectID/copy",            "projectCopy.remove",  "project-copy.ts" },
  { "POST",   "/experimental/project/:projectID/copy/refresh",    "projectCopy.refresh", "project-copy.ts" },
  { "GET", "/api/provider",                                "provider.list",         "provider.ts" },
  { "GET", "/api/provider/:providerID",                    "provider.get",          "provider.ts" },
  { "GET",    "/api/pty",                                  "pty.list",         "pty.ts" },
  { "POST",   "/api/pty",                                  "pty.create",       "pty.ts" },
  { "GET",    "/api/pty/:ptyID",                           "pty.get",          "pty.ts" },
  { "PUT",    "/api/pty/:ptyID",                           "pty.update",       "pty.ts" },
  { "DELETE", "/api/pty/:ptyID",                           "pty.remove",       "pty.ts" },
  { "POST",   "/api/pty/:ptyID/connect-token",             "pty.connectToken", "pty.ts" },
  { "GET",    "/api/pty/:ptyID/connect",                   "pty.connect",      "pty.ts" },
  { "GET",  "/api/question/request",                              "question.request.list",  "question.ts" },
  { "GET",  "/api/session/:sessionID/question",                   "session.question.list",  "question.ts" },
  { "POST", "/api/session/:sessionID/question/:requestID/reply",  "session.question.reply", "question.ts" },
  { "POST", "/api/session/:sessionID/question/:requestID/reject", "session.question.reject","question.ts" },
  { "GET", "/api/reference",                               "reference.list",        "reference.ts" },
  { "GET",  "/api/session",                                "session.list",          "session.ts" },
  { "POST", "/api/session",                                "session.create",        "session.ts" },
  { "GET",  "/api/session/active",                         "session.active",        "session.ts" },
  { "GET",  "/api/session/:sessionID",                     "session.get",           "session.ts" },
  { "POST", "/api/session/:sessionID/agent",               "session.switchAgent",   "session.ts" },
  { "POST", "/api/session/:sessionID/model",               "session.switchModel",   "session.ts" },
  { "POST", "/api/session/:sessionID/prompt",              "session.prompt",        "session.ts" },
  { "POST", "/api/session/:sessionID/compact",             "session.compact",       "session.ts" },
  { "POST", "/api/session/:sessionID/wait",                "session.wait",          "session.ts" },
  { "POST", "/api/session/:sessionID/revert/stage",        "session.revert.stage",  "session.ts" },
  { "POST", "/api/session/:sessionID/revert/clear",        "session.revert.clear",  "session.ts" },
  { "POST", "/api/session/:sessionID/revert/commit",       "session.revert.commit", "session.ts" },
  { "GET",  "/api/session/:sessionID/context",             "session.context",       "session.ts" },
  { "GET",  "/api/session/:sessionID/history",             "session.history",       "session.ts" },
  { "GET",  "/api/session/:sessionID/event",               "session.events",        "session.ts" },
  { "POST", "/api/session/:sessionID/interrupt",           "session.interrupt",     "session.ts" },
  { "GET",  "/api/session/:sessionID/message/:messageID",  "session.message",       "session.ts" },
  { "GET", "/api/skill",                                   "skill.list",            "skill.ts" },
}

----------------------------------------------------------------------
-- Constants
----------------------------------------------------------------------

local DOC_DIR      = "docs/opencode-protocol/audits/endpoints"
local OPENCODE_SRC = "C:/Users/heycj/dev/opencode"
local MODEL        = "minimax-cn-coding-plan/MiniMax-M3"

----------------------------------------------------------------------
-- Schemas
----------------------------------------------------------------------

local AUDIT_SCHEMA = {
  type = "object",
  properties = {
    identifier       = { type = "string" },
    method           = { type = "string" },
    path             = { type = "string" },
    group            = { type = "string" },
    conformance_level = { type = "string", description = "conformant | partial | non_conformant | not_implemented" },
    dimensions = {
      type = "object",
      description = "8 audit dimensions, each with status + notes",
      properties = {
        route_registration = { type = "object", properties = { status = { type = "string" }, notes = { type = "string" } } },
        response_shape     = { type = "object", properties = { status = { type = "string" }, notes = { type = "string" } } },
        status_code        = { type = "object", properties = { status = { type = "string" }, notes = { type = "string" } } },
        error_handling     = { type = "object", properties = { status = { type = "string" }, notes = { type = "string" } } },
        payload_validation = { type = "object", properties = { status = { type = "string" }, notes = { type = "string" } } },
        query_params       = { type = "object", properties = { status = { type = "string" }, notes = { type = "string" } } },
        middleware         = { type = "object", properties = { status = { type = "string" }, notes = { type = "string" } } },
        side_effects       = { type = "object", properties = { status = { type = "string" }, notes = { type = "string" } } },
      },
    },
    deviations = {
      type = "array",
      items = {
        type = "object",
        properties = {
          id                = { type = "string" },
          dimension         = { type = "string" },
          severity          = { type = "string", description = "critical | high | medium | low" },
          title             = { type = "string" },
          contract_says     = { type = "string" },
          loom_does         = { type = "string" },
          evidence_contract = { type = "string" },
          evidence_loom     = { type = "string" },
          impact            = { type = "string" },
        },
        required = { "id", "dimension", "severity", "title", "contract_says", "loom_does", "evidence_contract", "evidence_loom" },
      },
    },
    missing_features = { type = "array", items = { type = "string" } },
    extra_features   = { type = "array", items = { type = "string" } },
    notes            = { type = "string" },
  },
  required = { "identifier", "conformance_level", "dimensions", "deviations" },
}

local ADVERSARY_SCHEMA = {
  type = "object",
  properties = {
    identifier   = { type = "string" },
    angle        = { type = "string", description = "schema | behavior | boundary" },
    reviewed_deviations = {
      type = "array",
      items = {
        type = "object",
        properties = {
          id                  = { type = "string" },
          verdict             = { type = "string", description = "confirmed | rejected | corrected" },
          reviewer_notes      = { type = "string" },
          corrected_severity  = { type = "string" },
        },
        required = { "id", "verdict", "reviewer_notes" },
      },
    },
    new_deviations_found = {
      type = "array",
      items = {
        type = "object",
        properties = {
          dimension         = { type = "string" },
          severity          = { type = "string" },
          title             = { type = "string" },
          contract_says     = { type = "string" },
          loom_does         = { type = "string" },
          evidence_contract = { type = "string" },
          evidence_loom     = { type = "string" },
          impact            = { type = "string" },
        },
        required = { "dimension", "severity", "title", "contract_says", "loom_does", "evidence_contract", "evidence_loom" },
      },
    },
    overall_assessment = { type = "string" },
  },
  required = { "identifier", "angle", "reviewed_deviations", "new_deviations_found", "overall_assessment" },
}

local DOC_SCHEMA = {
  type = "object",
  properties = {
    identifier       = { type = "string" },
    doc_path         = { type = "string" },
    doc_written      = { type = "boolean" },
    conformance_level = { type = "string" },
    deviation_count  = { type = "integer" },
    summary_one_line = { type = "string" },
  },
  required = { "identifier", "doc_path", "doc_written", "conformance_level", "deviation_count", "summary_one_line" },
}

local INDEX_SCHEMA = {
  type = "object",
  properties = {
    index_path    = { type = "string" },
    index_written = { type = "boolean" },
    doc_count     = { type = "integer" },
  },
  required = { "index_path", "index_written", "doc_count" },
}

----------------------------------------------------------------------
-- Prompt builders
----------------------------------------------------------------------

local function audit_prompt(idx, method, path, identifier, group_file)
  return table.concat({
    "FINAL action: call structured_output. NEVER answer as plain text.",
    "",
    "You are the CONFORMANCE AUDITOR for ONE opencode v2 HTTP endpoint.",
    "Your job: read BOTH the opencode contract AND the Loom implementation,",
    "then systematically check 8 dimensions for deviations.",
    "",
    "ENDPOINT: " .. identifier,
    "METHOD:   " .. method,
    "PATH:     " .. path,
    "GROUP:    " .. group_file,
    "",
    "CONTRACT SOURCES:",
    "  Protocol group:  " .. OPENCODE_SRC .. "/packages/protocol/src/groups/" .. group_file,
    "  Protocol errors: " .. OPENCODE_SRC .. "/packages/protocol/src/errors.ts",
    "  Schema dir:      " .. OPENCODE_SRC .. "/packages/schema/src/",
    "  Server handler:  " .. OPENCODE_SRC .. "/packages/server/src/handlers/" .. group_file,
    "",
    "LOOM SOURCES:",
    "  Routes:          apps/server/src/routes.rs",
    "  Handlers:        apps/server/src/handlers/*.rs",
    "  State:           apps/server/src/state.rs",
    "  SSE:             apps/server/src/sse.rs",
    "",
    "Existing endpoint doc for reference (do NOT blindly trust it — verify yourself):",
    "  docs/opencode-protocol/specs/endpoints/v2." .. identifier .. ".md",
    "",
    "TASK:",
    "1. Read the contract: " .. OPENCODE_SRC .. "/packages/protocol/src/groups/" .. group_file,
    "   - Extract the HttpApiEndpoint definition for this endpoint",
    "   - Note: success schema, error classes, payload, query, params, middleware annotations",
    "2. Read the schema files referenced by the contract",
    "3. Grep apps/server/src/routes.rs for the route path to find registration",
    "4. Read the Loom handler file that processes this endpoint",
    "5. Check ALL 8 dimensions:",
    "",
    "  D1. ROUTE REGISTRATION",
    "    - Is the route registered in routes.rs?",
    "    - Does the path match the contract exactly (including :params)?",
    "    - Does the HTTP method match?",
    "    - Is it a real handler, a placeholder (v2_compat), or a TODO comment?",
    "",
    "  D2. RESPONSE SHAPE",
    "    - Does Loom's response match the contract's success schema?",
    "    - Is Location.response wrapping correct?",
    "    - Are all fields present with correct types?",
    "    - Are tagged unions properly discriminated?",
    "    - Are optional fields handled (absent vs null)?",
    "",
    "  D3. STATUS CODE",
    "    - Does the success status match (200 vs 204)?",
    "    - Does NoContent map to 204 with empty body?",
    "",
    "  D4. ERROR HANDLING",
    "    - Are all contract-declared error classes actually returned?",
    "    - Do error response bodies match the declared schema?",
    "    - Are 404/400/500 bodies shaped correctly?",
    "    - Are missing-relationship errors (SessionNotFoundError etc.) returned?",
    "",
    "  D5. PAYLOAD VALIDATION",
    "    - Are required fields validated?",
    "    - Are optional fields defaulted correctly?",
    "    - Are type constraints enforced (branded strings, ranges)?",
    "",
    "  D6. QUERY / PARAMS",
    "    - Is LocationQuery accepted where the contract expects it?",
    "    - Are path params extracted correctly?",
    "    - Is cursor pagination handled correctly?",
    "    - Are brand constraints on IDs enforced?",
    "",
    "  D7. MIDDLEWARE",
    "    - Is auth middleware applied where the contract expects it?",
    "    - Is sessionLocationMiddleware / locationMiddleware applied?",
    "    - Is locationQueryOpenApi annotation honored?",
    "",
    "  D8. SIDE EFFECTS",
    "    - Does Loom emit the expected SSE events?",
    "    - Does Loom persist state changes?",
    "    - Are idempotency semantics respected?",
    "    - Are state mutations atomic?",
    "",
    "6. For each deviation found, record:",
    "   - id: DEV-1, DEV-2, ...",
    "   - dimension: which of D1-D8",
    "   - severity: critical (breaks client) / high (wrong behavior) / medium (edge case) / low (cosmetic)",
    "   - contract_says: what the contract requires",
    "   - loom_does: what Loom actually does",
    "   - evidence_contract: file:line in opencode source",
    "   - evidence_loom: file:line in Loom source",
    "   - impact: what breaks if a client relies on the contract",
    "",
    "7. Set conformance_level:",
    "   - conformant: 0 deviations",
    "   - partial: 1+ deviations but endpoint is usable",
    "   - non_conformant: endpoint exists but response is fundamentally wrong",
    "   - not_implemented: route not registered at all",
    "",
    "Return the JSON object. Set identifier=\"" .. identifier .. "\".",
    "",
    "FINAL action: call structured_output. NEVER answer as plain text.",
  }, "\n")
end

local ADVERSARY_FOCI = {
  schema = table.concat({
    "ADVERSARY ANGLE: SCHEMA CONFORMANCE",
    "Your focus is response shape and type system conformance.",
    "Specifically verify:",
    "  - Response JSON structure matches contract success schema field-by-field",
    "  - Location.response wrapping (is { location, data } envelope present/absent correctly?)",
    "  - Tagged union variants: are ALL discriminator values handled?",
    "  - Optional fields: absent vs null vs default — does Loom match contract?",
    "  - Brand constraints on IDs (ses_, per_, msg_, pty_, que_, con_, cred_, psv_, evt_)",
    "  - Nested type chains: resolve ALL levels, not just top-level",
    "  - Array vs Map vs Object: does Loom return the correct collection type?",
    "  - Number types: Finite vs Int vs NonNegativeInt — does Loom respect constraints?",
  }, "\n"),
  behavior = table.concat({
    "ADVERSARY ANGLE: RUNTIME BEHAVIOR",
    "Your focus is behavioral correctness and side effects.",
    "Specifically verify:",
    "  - SSE events: does Loom emit the events the contract implies?",
    "  - Persistence: are mutations persisted? Is data lost on restart?",
    "  - Middleware: is auth/location/sessionLocation applied where contract expects?",
    "  - Idempotency: are repeat calls safe? Does clear/remove work if nothing staged?",
    "  - Ordering: does Loom respect order=asc/desc? Does cursor advance correctly?",
    "  - Timeout behavior: does Loom block or return immediately where contract is ambiguous?",
    "  - Error coercion: does Loom map internal errors to contract error classes correctly?",
    "  - Auth bypass: does Loom skip auth where contract says (e.g. health, pty.connect)?",
    "  - State leakage: does Loom accidentally expose cross-session data?",
  }, "\n"),
  boundary = table.concat({
    "ADVERSARY ANGLE: EDGE CASES & BOUNDARY CONDITIONS",
    "Your focus is error paths, validation gaps, and boundary conditions.",
    "Specifically verify:",
    "  - All contract-declared error classes: are they actually reachable in Loom?",
    "  - 404 body shape: does Loom return the contract's error schema or plain text?",
    "  - Missing/empty/null input: what does Loom do vs what contract requires?",
    "  - Concurrent access: are there race conditions in RwLock-protected state?",
    "  - cursor + order combination: does Loom validate mutually exclusive params?",
    "  - Whitespace-only strings: does Loom validate beyond schema (stricter or looser)?",
    "  - Unknown IDs: does Loom return 404, 200+null, or 500?",
    "  - Large payloads: does Loom enforce limit constraints?",
    "  - Stale TODO comments: does routes.rs have TODO(W2) comments that are already wired?",
    "  - Placeholder handlers: does v2_compat::true_value / empty_list / empty_object",
    "    return shapes that deviate from the contract?",
  }, "\n"),
}

local function adversary_prompt(idx, angle, method, path, identifier, group_file, audit_json)
  return table.concat({
    "FINAL action: call structured_output. NEVER answer as plain text.",
    "",
    "You are the ADVERSARIAL REVIEWER for ONE opencode v2 HTTP endpoint.",
    "Your job: independently verify the auditor's findings by re-reading the code.",
    "",
    "ENDPOINT: " .. identifier,
    "METHOD:   " .. method,
    "PATH:     " .. path,
    "",
    ADVERSARY_FOCI[angle] or ADVERSARY_FOCI.schema,
    "",
    "CONTRACT SOURCES:",
    "  " .. OPENCODE_SRC .. "/packages/protocol/src/groups/" .. group_file,
    "  " .. OPENCODE_SRC .. "/packages/protocol/src/errors.ts",
    "  " .. OPENCODE_SRC .. "/packages/schema/src/",
    "",
    "LOOM SOURCES:",
    "  apps/server/src/routes.rs",
    "  apps/server/src/handlers/*.rs",
    "  apps/server/src/state.rs",
    "",
    "=== AUDITOR'S FINDINGS (JSON) ===",
    audit_json,
    "=== END ===",
    "",
    "TASK:",
    "1. Re-read the contract and Loom source code for this endpoint.",
    "2. For EACH deviation the auditor found:",
    "   - Verify the evidence: does the cited file:line actually say what the auditor claims?",
    "   - Check the severity: is it correctly classified?",
    "   - Verdict: confirmed / rejected / corrected",
    "3. Find NEW deviations the auditor missed:",
    "   - Focus on your angle (" .. angle .. ")",
    "   - Common misses: stale TODO comments, v2_compat placeholders,",
    "     missing SSE events, wrong 404 body shape, concurrent access issues",
    "4. Do NOT rubber-stamp the auditor. Be skeptical.",
    "   - If the auditor says 'conformant', look harder for hidden issues.",
    "   - If the auditor says 'non_conformant', verify it's not a false positive.",
    "",
    "Return the JSON object with your verdicts and new findings.",
    "Set identifier=\"" .. identifier .. "\", angle=\"" .. angle .. "\".",
    "",
    "FINAL action: call structured_output. NEVER answer as plain text.",
  }, "\n")
end

local function doc_prompt(idx, method, path, identifier, group_file, audit_json, adv_schema_json, adv_behavior_json, adv_boundary_json)
  local doc_path = DOC_DIR .. "/v2." .. identifier .. ".md"
  return table.concat({
    "FINAL action: write the markdown file, then call structured_output.",
    "",
    "You are the DOC WRITER for a conformance audit report.",
    "Merge audit + adversary findings into a final report.",
    "",
    "ENDPOINT: " .. identifier,
    "METHOD:   " .. method,
    "PATH:     " .. path,
    "DOC PATH: " .. doc_path,
    "",
    "=== AUDIT (JSON) ===",
    audit_json,
    "=== END ===",
    "",
    "=== ADVERSARY: SCHEMA (JSON) ===",
    adv_schema_json or '{"error":"adversary failed"}',
    "=== END ===",
    "",
    "=== ADVERSARY: BEHAVIOR (JSON) ===",
    adv_behavior_json or '{"error":"adversary failed"}',
    "=== END ===",
    "",
    "=== ADVERSARY: BOUNDARY (JSON) ===",
    adv_boundary_json or '{"error":"adversary failed"}',
    "=== END ===",
    "",
    "TASK:",
    "1. Write a complete conformance report to: " .. doc_path,
    "2. Merge all deviations:",
    "   - Start with audit deviations",
    "   - Apply adversary verdicts (confirmed/rejected/corrected)",
    "   - Add new deviations found by adversaries",
    "   - Deduplicate overlapping findings",
    "3. Final deviation list = confirmed audit deviations + new adversary findings",
    "",
    "DOCUMENT STRUCTURE:",
    "",
    "# Conformance: `" .. identifier .. "`",
    "",
    "> Level: **<CONFORMANT|PARTIAL|NON_CONFORMANT|NOT_IMPLEMENTED>** · Deviations: N (X critical, Y high, Z medium, W low)",
    "",
    "## Summary table",
    "",
    "| Dimension | Status |",
    "| --- | --- |",
    "| Route registration | ✓/✗/⚠ |",
    "| Response shape | ✓/✗/⚠ |",
    "| Status code | ✓/✗/⚠ |",
    "| Error handling | ✓/✗/⚠ |",
    "| Payload validation | ✓/✗/⚠ |",
    "| Query/params | ✓/✗/⚠ |",
    "| Middleware | ✓/✗/⚠ |",
    "| Side effects | ✓/✗/⚠ |",
    "",
    "## Deviations",
    "",
    "### DEV-N: <title> [SEVERITY]",
    "- **Dimension**: D1-D8",
    "- **Contract says**: ...",
    "- **Loom does**: ...",
    "- **Contract ref**: file:line",
    "- **Loom ref**: file:line",
    "- **Impact**: ...",
    "- **Adversary verdict**: confirmed/rejected/corrected by <angle>",
    "",
    "(repeat for each deviation)",
    "",
    "## Adversary findings",
    "",
    "### Schema reviewer",
    "- Reviewed N deviations: X confirmed, Y rejected, Z corrected",
    "- Found N new deviations",
    "",
    "### Behavior reviewer",
    "(same)",
    "",
    "### Boundary reviewer",
    "(same)",
    "",
    "## Recommendations",
    "",
    "1. (actionable fix for each confirmed deviation, ordered by severity)",
    "",
    "## Source references",
    "- Contract: `" .. OPENCODE_SRC .. "/packages/protocol/src/groups/" .. group_file .. "`",
    "- Loom handler: `apps/server/src/handlers/...`",
    "",
    "Set conformance_level, deviation_count, and summary_one_line from the merged findings.",
    "",
    "FINAL action: call structured_output. NEVER answer as plain text.",
  }, "\n")
end

local function index_prompt(all_summaries_json)
  return table.concat({
    "FINAL action: write the index file, then call structured_output.",
    "",
    "You are the INDEX WRITER for the conformance audit.",
    "Use the summaries JSON to build README.md.",
    "Do NOT read individual audit files — all data is in the JSON.",
    "",
    "INDEX PATH: " .. DOC_DIR .. "/README.md",
    "",
    "=== SUMMARIES JSON ===",
    all_summaries_json,
    "=== END ===",
    "",
    "Write " .. DOC_DIR .. "/README.md with:",
    "1. Title and description",
    "2. Overall conformance stats (X conformant, Y partial, Z non_conformant, W not_implemented)",
    "3. Top 10 deviations by severity (across all endpoints)",
    "4. Per-group summary table",
    "5. Full endpoint table with: Method | Path | Identifier | Level | Deviations | Summary",
    "   Link identifier to ./v2.<identifier>.md",
    "6. Cross-references to ../LOOM-TOOL-PROTOCOL-GAPS.md and ../../specs/endpoints/README.md",
    "",
    "FINAL action: call structured_output. NEVER answer as plain text.",
  }, "\n")
end

----------------------------------------------------------------------
-- Fan-out runner with retry
----------------------------------------------------------------------

local function run_fanout(specs, label)
  local results = {}
  local pending = {}
  for i = 1, #specs do
    pending[i] = specs[i]
    results[i] = nil
  end

  for attempt = 1, 3 do
    if #pending == 0 then break end
    log(string.format("[%s] attempt %d/3: %d agents remaining", label, attempt, #pending))

    local ok_p, res = pcall(parallel, pending, function(r) return r end)
    if not ok_p or type(res) ~= "table" then
      log(string.format("[%s] parallel() failed: %s", label, tostring(res)), "warn")
      break
    end

    local still_failing = {}
    for i = 1, #pending do
      local r = res[i]
      if r and r.ok and type(r.output) == "table" then
        for orig_i = 1, #specs do
          if specs[orig_i].name == pending[i].name then
            results[orig_i] = r.output
            break
          end
        end
      else
        if attempt < 3 then
          table.insert(still_failing, pending[i])
          log(string.format("  %s: failed attempt %d, will retry", pending[i].name, attempt), "warn")
        else
          log(string.format("  %s: FAILED after 3 attempts", pending[i].name), "warn")
        end
      end
    end
    pending = still_failing
  end

  return results
end

----------------------------------------------------------------------
-- Main
----------------------------------------------------------------------

function main()
  local N = #ENDPOINTS
  log(string.format("Conformance audit: %d endpoints, %d agents (61 audit + 183 adversary + 62 doc/index)",
    N, N + N * 3 + N + 1))

  ------------------------------------------------------------
  -- PHASE 1: AUDIT (61 agents, cap=4)
  ------------------------------------------------------------
  phase("audit", N)
  log(string.format("PHASE 1 AUDIT: launching %d audit agents...", N))

  local audit_specs = {}
  for i, ep in ipairs(ENDPOINTS) do
    local method, path, identifier, group_file = ep[1], ep[2], ep[3], ep[4]
    table.insert(audit_specs, {
      name   = "audit-" .. string.format("%02d", i),
      prompt = audit_prompt(i, method, path, identifier, group_file),
      schema = AUDIT_SCHEMA,
      model  = MODEL,
    })
  end

  local audits = run_fanout(audit_specs, "audit")

  local audit_ok, audit_fail = 0, 0
  for i = 1, N do
    if audits[i] then audit_ok = audit_ok + 1 else audit_fail = audit_fail + 1 end
  end
  log(string.format("PHASE 1 complete: %d ok, %d failed", audit_ok, audit_fail))

  ------------------------------------------------------------
  -- PHASE 2: ADVERSARY (183 agents, cap=4)
  ------------------------------------------------------------
  local angles = { "schema", "behavior", "boundary" }
  phase("adversary", N * 3)
  log(string.format("PHASE 2 ADVERSARY: launching %d adversary agents...", N * 3))

  local adv_specs = {}
  for i, ep in ipairs(ENDPOINTS) do
    local method, path, identifier, group_file = ep[1], ep[2], ep[3], ep[4]
    local audit_json = audits[i] and json.encode(audits[i]) or '{"identifier":"' .. identifier .. '","error":"audit failed"}'
    for _, angle in ipairs(angles) do
      table.insert(adv_specs, {
        name   = string.format("adversary-%s-%02d", angle, i),
        prompt = adversary_prompt(i, angle, method, path, identifier, group_file, audit_json),
        schema = ADVERSARY_SCHEMA,
        model  = MODEL,
      })
    end
  end

  local adv_results = run_fanout(adv_specs, "adversary")

  -- Group adversary results by endpoint index
  local adv_grouped = {}
  for i = 1, N do
    adv_grouped[i] = {
      schema   = adv_results[(i - 1) * 3 + 1],
      behavior = adv_results[(i - 1) * 3 + 2],
      boundary = adv_results[(i - 1) * 3 + 3],
    }
  end

  local adv_ok, adv_fail = 0, 0
  for i = 1, #(adv_specs) do
    if adv_results[i] then adv_ok = adv_ok + 1 else adv_fail = adv_fail + 1 end
  end
  log(string.format("PHASE 2 complete: %d ok, %d failed", adv_ok, adv_fail))

  ------------------------------------------------------------
  -- PHASE 3: DOC WRITE + INDEX (62 agents, cap=4)
  ------------------------------------------------------------
  phase("doc-write", N + 1)
  log(string.format("PHASE 3 DOC-WRITE: launching %d doc agents + 1 index...", N))

  local doc_specs = {}
  for i, ep in ipairs(ENDPOINTS) do
    local method, path, identifier, group_file = ep[1], ep[2], ep[3], ep[4]
    local audit_json = audits[i] and json.encode(audits[i]) or '{}'
    local ag = adv_grouped[i]
    local adv_s = ag.schema and json.encode(ag.schema) or '{}'
    local adv_b = ag.behavior and json.encode(ag.behavior) or '{}'
    local adv_bd = ag.boundary and json.encode(ag.boundary) or '{}'

    table.insert(doc_specs, {
      name   = "doc-" .. string.format("%02d", i),
      prompt = doc_prompt(i, method, path, identifier, group_file, audit_json, adv_s, adv_b, adv_bd),
      schema = DOC_SCHEMA,
      model  = MODEL,
    })
  end

  local docs = run_fanout(doc_specs, "doc-write")

  local doc_ok, doc_fail = 0, 0
  local doc_summaries = {}
  for i = 1, N do
    if docs[i] and docs[i].doc_written then
      doc_ok = doc_ok + 1
      local ep = ENDPOINTS[i]
      table.insert(doc_summaries, {
        identifier        = ep[3],
        method            = ep[1],
        path              = ep[2],
        group             = ep[4]:gsub("%.ts$", ""),
        conformance_level = docs[i].conformance_level or "unknown",
        deviation_count   = docs[i].deviation_count or 0,
        summary           = docs[i].summary_one_line or "",
      })
    else
      doc_fail = doc_fail + 1
    end
  end
  log(string.format("PHASE 3 doc-write complete: %d ok, %d failed", doc_ok, doc_fail))

  -- Index agent
  log("PHASE 3 INDEX: building index...")

  local summaries_json = json.encode(doc_summaries)

  local index_res = agent({
    name   = "index-build",
    prompt = index_prompt(summaries_json),
    schema = INDEX_SCHEMA,
    model  = MODEL,
  })

  local index_ok = index_res.ok and index_res.output and index_res.output.index_written
  log(string.format("PHASE 3 index complete: %s", index_ok and "written" or "FAILED"))

  ------------------------------------------------------------
  -- Report
  ------------------------------------------------------------
  report({
    pipeline       = "opencode-conformance-audit",
    status         = "completed",
    endpoint_count = N,
    total_agents   = N + N * 3 + N + 1,
    phases = {
      audit     = { total = N,     ok = audit_ok, failed = audit_fail },
      adversary = { total = N * 3, ok = adv_ok,   failed = adv_fail },
      doc_write = { total = N,     ok = doc_ok,   failed = doc_fail },
      index     = { total = 1,     ok = index_ok and 1 or 0, failed = index_ok and 0 or 1 },
    },
    doc_directory = DOC_DIR,
    endpoints     = doc_summaries,
  })
end