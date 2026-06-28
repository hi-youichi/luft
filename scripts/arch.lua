--------------------------------------------
-- Goal:  Review overall code architecture and produce prioritized recommendations
-- Arch:
--   discover subsystems                       --> [subsystems[]]
--   (pipeline) per subsystem (max_inflight 4):
--     +-- overview (structure, API, deps)     --> [ARCH_OVERVIEW]
--     \-- assess (quality, issues, scores)    --> [ARCH_ASSESS]
--   cross-cutting analysis                    --> [CROSS_CUT]
--   (parallel) adversarial verify findings    --> [verified[]]
--   \-- synthesize report                     --> [REPORT]
-- Flow:  discover -> subsystems[] -> pipeline(overview->assess) -> cross-cut -> verify -> report
--------------------------------------------

-- ── Schemas ──

local SUBSYSTEMS_SCHEMA = {
  type = "object",
  properties = {
    subsystems = {
      type = "array",
      items = {
        type = "object",
        properties = {
          name        = { type = "string" },
          path        = { type = "string" },
          description = { type = "string" },
          estimated_loc = { type = "integer" }
        },
        required = { "name", "path", "description" }
      }
    },
    summary = { type = "string" }
  },
  required = { "subsystems" }
}

local ARCH_OVERVIEW_SCHEMA = {
  type = "object",
  properties = {
    module             = { type = "string" },
    path               = { type = "string" },
    responsibilities   = { type = "array", items = { type = "string" } },
    public_api         = { type = "array", items = { type = "string" } },
    internal_structure = { type = "string" },
    key_types          = { type = "array", items = { type = "string" } },
    dependencies       = { type = "array", items = { type = "string" } },
    dependents         = { type = "array", items = { type = "string" } }
  },
  required = { "module", "path", "responsibilities", "internal_structure" }
}

local ARCH_ASSESS_SCHEMA = {
  type = "object",
  properties = {
    module   = { type = "string" },
    findings = {
      type = "array",
      items = {
        type = "object",
        properties = {
          severity       = { type = "string", enum = { "critical", "high", "medium", "low", "info" } },
          category       = { type = "string" },
          description    = { type = "string" },
          recommendation = { type = "string" }
        },
        required = { "severity", "category", "description", "recommendation" }
      }
    },
    cohesion_score     = { type = "integer" },
    coupling_score     = { type = "integer" },
    testability_score  = { type = "integer" },
    summary            = { type = "string" }
  },
  required = { "module", "findings", "summary" }
}

local CROSS_CUT_SCHEMA = {
  type = "object",
  properties = {
    dependency_graph      = { type = "string" },
    circular_deps         = { type = "array", items = { type = "string" } },
    consistency_issues    = {
      type = "array",
      items = {
        type = "object",
        properties = {
          area           = { type = "string" },
          issue          = { type = "string" },
          recommendation = { type = "string" }
        },
        required = { "area", "issue" }
      }
    },
    architectural_patterns = { type = "array", items = { type = "string" } },
    tech_debt = {
      type = "array",
      items = {
        type = "object",
        properties = {
          area        = { type = "string" },
          severity    = { type = "string" },
          description = { type = "string" }
        },
        required = { "area", "description" }
      }
    },
    summary = { type = "string" }
  },
  required = { "summary" }
}

local VOTE_SCHEMA = {
  type = "object",
  properties = {
    approve = { type = "boolean" },
    reason  = { type = "string" }
  },
  required = { "approve", "reason" }
}

local REPORT_SCHEMA = {
  type = "object",
  properties = {
    executive_summary = { type = "string" },
    strengths         = { type = "array", items = { type = "string" } },
    weaknesses        = { type = "array", items = { type = "string" } },
    recommendations = {
      type = "array",
      items = {
        type = "object",
        properties = {
          priority = { type = "string", enum = { "P0", "P1", "P2", "P3" } },
          area     = { type = "string" },
          action   = { type = "string" }
        },
        required = { "priority", "area", "action" }
      }
    },
    risk_assessment = { type = "string" },
    coverage        = { type = "string" }
  },
  required = { "executive_summary", "recommendations" }
}

-- ── Phase 1: Discover subsystems ──

phase("discover subsystems", 1)

local focus = args.focus or "overall architecture"

local discover = agent({
  prompt = "You are reviewing the code architecture of a Rust workspace (Maestro multi-agent workflow runtime).\n\n"
    .. "Task: Explore the codebase structure and enumerate all major subsystems/modules.\n\n"
    .. "Steps:\n"
    .. "1. Read the workspace root Cargo.toml to identify all crates\n"
    .. "2. For each crate, list its top-level modules (src/*.rs and src/*/mod.rs)\n"
    .. "3. Group related modules into logical subsystems\n"
    .. "4. For each subsystem, note: name, source path, brief description, estimated lines of code\n\n"
    .. "Be thorough — cover all crates and significant modules. Include CLI, TUI, runtime, core, "
    .. "contract, scheduler, state, adapters, MCP, planner, and any other subsystems you find.\n\n"
    .. "Focus area: " .. focus,
  schema = SUBSYSTEMS_SCHEMA
})

if not discover.ok then
  report({ error = "discover failed: " .. (discover.status or "unknown") })
  return
end

local subsystems = discover.output.subsystems or {}
log("discovered " .. #subsystems .. " subsystems")

if #subsystems == 0 then
  report({ error = "no subsystems discovered" })
  return
end

-- ── Phase 2: Per-subsystem pipeline (overview -> assess) ──

local pipeline_span = phase_begin("analyze subsystems", #subsystems)

local pipe_results = pipeline{
  items = subsystems,
  max_inflight = 4,
  stages = {
    -- Stage 1: Structural overview
    function(subsys)
      phase("overview " .. subsys.name)
      return {
        prompt = "Analyze the architecture of the \"" .. subsys.name .. "\" subsystem at "
          .. subsys.path .. ".\n\n"
          .. "This is a Rust codebase (Maestro multi-agent workflow runtime). "
          .. "Read the actual source files and document:\n"
          .. "1. Core responsibilities — what this subsystem does\n"
          .. "2. Public API — key types, traits, functions exposed to other modules\n"
          .. "3. Internal structure — how the code is organized internally\n"
          .. "4. Key types — important structs/enums/traits\n"
          .. "5. Dependencies — what other modules/crates this depends on\n"
          .. "6. Dependents — what other modules depend on this\n\n"
          .. "Read the real source code. Do not guess.",
        schema = ARCH_OVERVIEW_SCHEMA
      }
    end,
    -- Stage 2: Quality assessment
    function(prev)
      local mod_name = (prev.ok and prev.output and prev.output.module) or "unknown"
      phase("assess " .. mod_name)
      if not prev.ok then
        return {
          prompt = "Return a minimal assessment: module=\"unknown\", findings=[], "
            .. "cohesion_score=0, coupling_score=0, testability_score=0, "
            .. "summary=\"overview stage failed\"",
          schema = ARCH_ASSESS_SCHEMA
        }
      end
      return {
        prompt = "Assess the architecture quality of the \"" .. prev.output.module
          .. "\" module.\n\n"
          .. "Overview data:\n" .. json.encode(prev.output) .. "\n\n"
          .. "Read the actual source code at " .. (prev.output.path or "?")
          .. " and evaluate:\n"
          .. "1. Cohesion — does the module have a single clear responsibility?\n"
          .. "2. Coupling — how tightly coupled is it to other modules?\n"
          .. "3. Testability — how easy is it to test in isolation?\n"
          .. "4. Error handling — is error propagation consistent and robust?\n"
          .. "5. Abstraction quality — are abstractions appropriate (not leaky, not over-engineered)?\n"
          .. "6. Code organization — is the internal structure clean?\n\n"
          .. "Score each dimension 0-10 (10 = excellent). "
          .. "List ALL findings with severity and actionable recommendations.",
        schema = ARCH_ASSESS_SCHEMA
      }
    end
  }
}

phase_end(pipeline_span)

-- Collect successful assessments
local assessments = {}
for i, r in ipairs(pipe_results.items or {}) do
  if r.ok and r.output then
    table.insert(assessments, r.output)
  else
    log("pipeline item " .. i .. " failed: " .. (r.status or "unknown"), "warn")
  end
end

log("collected " .. #assessments .. " assessments")

if #assessments == 0 then
  report({ error = "all subsystem analyses failed", subsystems_discovered = #subsystems })
  return
end

-- ── Phase 3: Cross-cutting analysis ──

phase("cross-cutting analysis", 1)

local cross = agent({
  prompt = "You are performing a cross-cutting architecture analysis of a Rust workspace "
    .. "(Maestro multi-agent workflow runtime).\n\n"
    .. "Per-subsystem assessments:\n" .. json.encode(assessments) .. "\n\n"
    .. "Analyze the following cross-cutting concerns by reading the ACTUAL source code:\n"
    .. "1. Dependency graph — trace inter-module dependencies, identify cycles or "
    .. "overly coupled pairs\n"
    .. "2. Consistency — check for inconsistent patterns (error handling, naming, "
    .. "API conventions, state management, async patterns)\n"
    .. "3. Architectural patterns — what patterns are used? Are they appropriate?\n"
    .. "4. Technical debt — areas that need refactoring or pose maintenance risk\n\n"
    .. "Focus on the most impactful cross-cutting issues.",
  schema = CROSS_CUT_SCHEMA
})

if not cross.ok then
  log("cross-cutting analysis failed: " .. (cross.status or "unknown"), "warn")
end

-- ── Phase 4: Adversarial verification of critical findings ──

-- Gather critical/high findings from per-subsystem assessments
local critical_findings = {}
for _, a in ipairs(assessments) do
  for _, f in ipairs(a.findings or {}) do
    if f.severity == "critical" or f.severity == "high" then
      table.insert(critical_findings, {
        module        = a.module,
        severity      = f.severity,
        category      = f.category,
        description   = f.description,
        recommendation = f.recommendation
      })
    end
  end
end

-- Add cross-cutting issues and tech debt
if cross.ok and cross.output then
  for _, iss in ipairs(cross.output.consistency_issues or {}) do
    table.insert(critical_findings, {
      module        = "cross-cutting",
      severity      = "high",
      category      = iss.area,
      description   = iss.issue,
      recommendation = iss.recommendation or ""
    })
  end
  for _, td in ipairs(cross.output.tech_debt or {}) do
    table.insert(critical_findings, {
      module        = "cross-cutting",
      severity      = td.severity or "high",
      category      = td.area,
      description   = td.description,
      recommendation = ""
    })
  end
end

log("collected " .. #critical_findings .. " critical/high findings for verification")

local verified_findings = {}

if #critical_findings > 0 then
  phase("verify findings", math.min(#critical_findings, 16))

  -- Bound concurrent verification to 16
  local to_verify = critical_findings
  if #to_verify > 16 then
    to_verify = {}
    for i = 1, 16 do
      table.insert(to_verify, critical_findings[i])
    end
    log("capped verification to top 16 of " .. #critical_findings .. " findings", "warn")
  end

  local votes = parallel(to_verify, function(finding)
    return {
      prompt = "You are an adversarial reviewer verifying an architecture finding.\n\n"
        .. "Finding to verify:\n" .. json.encode(finding) .. "\n\n"
        .. "Read the ACTUAL source code in the codebase and determine:\n"
        .. "- Is this finding accurate? Does the code actually exhibit the described issue?\n"
        .. "- Is the severity appropriate, or is it overstated?\n"
        .. "- Is the recommendation actionable and correct?\n\n"
        .. "Approve ONLY if the finding is accurate and the severity is justified. "
        .. "Reject if the issue does not exist, is overstated, or the recommendation is wrong.",
      schema = VOTE_SCHEMA
    }
  end)

  for i, finding in ipairs(to_verify) do
    if votes[i].ok and votes[i].output.approve then
      table.insert(verified_findings, finding)
    end
  end

  log("verified: " .. #verified_findings .. "/" .. #to_verify .. " findings confirmed")
end

-- ── Phase 5: Synthesize final report ──

phase("synthesize report", 1)

local synthesis_input = {
  subsystem_count   = #subsystems,
  assessed_count    = #assessments,
  assessments       = assessments,
  cross_cutting     = cross.ok and cross.output or { error = cross.status or "failed" },
  verified_findings = verified_findings,
  rejected_count    = #critical_findings - #verified_findings
}

local final = agent({
  prompt = "You are the lead architect producing the final architecture review report "
    .. "for Maestro (Rust multi-agent workflow runtime).\n\n"
    .. "Review data:\n" .. json.encode(synthesis_input) .. "\n\n"
    .. "Produce a comprehensive architecture review:\n"
    .. "1. Executive summary — overall assessment of architecture health (2-3 paragraphs)\n"
    .. "2. Strengths — what the architecture does well\n"
    .. "3. Weaknesses — key architectural concerns\n"
    .. "4. Recommendations — prioritized:\n"
    .. "   P0 = urgent / blocking, P1 = important, P2 = improvement, P3 = nice-to-have\n"
    .. "5. Risk assessment — top risks and their impact\n"
    .. "6. Coverage — what was reviewed and any blind spots\n\n"
    .. "IMPORTANT: Focus on VERIFIED findings (adversarially confirmed). "
    .. "Only include P0/P1 items backed by evidence from the code review. "
    .. "Be specific and actionable — reference actual modules and code paths.",
  schema = REPORT_SCHEMA
})

if not final.ok then
  report({
    error               = "synthesis failed: " .. (final.status or "unknown"),
    subsystems_reviewed = #assessments,
    verified_findings   = verified_findings,
    raw_assessments     = assessments,
    cross_cutting       = cross.ok and cross.output or nil
  })
  return
end

-- Enrich report with metadata
final.output.subsystems_reviewed = #assessments
final.output.verified_findings   = verified_findings
final.output.cross_cutting       = cross.ok and cross.output or nil

report(final.output)