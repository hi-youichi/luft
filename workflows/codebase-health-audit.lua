----------------------------------------------------
-- Goal:  Audit Maestro codebase health and produce prioritized improvement plan
-- Arch:
--   +----------+
--   | discover |=====> (for each subsystem)
--   +----------+        |
--        |              +----------+        +---------+        +--------+
--        |              | analyze  |=======>| assess  |=======>| score  |--> [result]
--        |              +----------+        +---------+        +--------+
--        |              (retry <= 2)               (degrade on fail)
--        v
--   +----------+        +--------+
--   | triage   |=======>| report |
--   +----------+        +--------+
-- Flow:  discover -> subsystems[] -> pipeline(analyze->assess->score) -> triage -> report
----------------------------------------------------
meta = {
  reasoning = "Discover project subsystems, then pipeline each through analyze/assess/score, triage findings into a prioritized improvement plan.",
  phases = {
    { label = "discover", description = "Enumerate project subsystems and modules", agents = 1 },
    { label = "analyze", description = "Deep analysis of each subsystem for issues", agents = 8, depends_on = {1}, dynamic = true },
    { label = "assess", description = "Severity assessment and scoring per subsystem", agents = 8, depends_on = {1}, dynamic = true },
    { label = "triage", description = "Cross-cutting triage and prioritization", agents = 1, depends_on = {2, 3} },
    { label = "report", description = "Final consolidated health report", depends_on = {4} },
  },
}

local DISCOVER_SCHEMA = {
  type = "object",
  properties = {
    subsystems = {
      type = "array",
      items = {
        type = "object",
        properties = {
          name = { type = "string" },
          path = { type = "string" },
          purpose = { type = "string" },
          estimated_lines = { type = "integer" },
        },
        required = { "name", "path", "purpose" },
      },
    },
  },
  required = { "subsystems" },
}

local ANALYSIS_SCHEMA = {
  type = "object",
  properties = {
    subsystem = { type = "string" },
    issues = {
      type = "array",
      items = {
        type = "object",
        properties = {
          category = { type = "string" },
          description = { type = "string" },
          location = { type = "string" },
          severity = { type = "string", enum = { "critical", "high", "medium", "low" } },
          effort = { type = "string", enum = { "trivial", "small", "medium", "large" } },
        },
        required = { "category", "description", "severity", "effort" },
      },
    },
    strengths = { type = "array", items = { type = "string" } },
    test_coverage_estimate = { type = "string" },
  },
  required = { "subsystem", "issues" },
}

local ASSESS_SCHEMA = {
  type = "object",
  properties = {
    subsystem = { type = "string" },
    health_score = { type = "integer", minimum = 0, maximum = 100 },
    top_risks = { type = "array", items = { type = "string" } },
    recommended_actions = {
      type = "array",
      items = {
        type = "object",
        properties = {
          action = { type = "string" },
          priority = { type = "string", enum = { "P0", "P1", "P2", "P3" } },
          impact = { type = "string" },
        },
        required = { "action", "priority", "impact" },
      },
    },
  },
  required = { "subsystem", "health_score", "recommended_actions" },
}

local TRIAGE_SCHEMA = {
  type = "object",
  properties = {
    overall_health = { type = "integer", minimum = 0, maximum = 100 },
    executive_summary = { type = "string" },
    priority_matrix = {
      type = "array",
      items = {
        type = "object",
        properties = {
          quadrant = { type = "string" },
          items = { type = "array", items = { type = "string" } },
        },
        required = { "quadrant", "items" },
      },
    },
    quick_wins = { type = "array", items = { type = "string" } },
    strategic_initiatives = { type = "array", items = { type = "string" } },
  },
  required = { "overall_health", "executive_summary", "priority_matrix", "quick_wins" },
}

function main()
  phase("discover", 1)
  local d = agent({
    prompt = "You are analyzing the Maestro project, a Rust multi-agent workflow orchestration runtime.\n\n"
      .. "Enumerate ALL subsystems and key modules in this project. For each, provide:\n"
      .. "- name: short identifier\n"
      .. "- path: file path or directory\n"
      .. "- purpose: one-line description\n"
      .. "- estimated_lines: approximate line count\n\n"
      .. "Focus on: src/runtime/, src/core/, src/adapters/, src/storage/, src/service/, src/tui/, src/cli.rs, src/planner.rs, src/mcp.rs\n"
      .. "Also note any test directories and their coverage.",
    schema = DISCOVER_SCHEMA,
    name = "discover",
    description = "Enumerate project subsystems",
  })
  if not d.ok then
    report({ error = "discover failed: " .. (d.status or "unknown") })
    return
  end

  local subsystems = d.output.subsystems or {}
  if #subsystems == 0 then
    report({ error = "no subsystems discovered" })
    return
  end

  log("discovered " .. #subsystems .. " subsystems")

  local results = pipeline{
    items = subsystems,
    max_inflight = 4,
    stages = {
      {
        label = "analyze",
        handler = function(sub)
          local span_name = "analyze " .. sub.name
          if completed_spans and completed_spans[span_name] then
            log("skipping completed: " .. span_name)
            return { ok = true, output = { subsystem = sub.name, issues = {}, strengths = {} } }
          end
          phase("analyze " .. sub.name)
          local r = agent({
            prompt = "Perform a deep code health analysis of the \"" .. sub.name .. "\" subsystem (" .. sub.path .. ").\n\n"
              .. "Purpose: " .. sub.purpose .. "\n\n"
              .. "Look for:\n"
              .. "1. FIXME/TODO/HACK comments indicating known issues\n"
              .. "2. Dead code (#\[allow(dead_code)\], unused imports, unreachable branches)\n"
              .. "3. Error handling gaps (unwrap() on fallible ops, missing error propagation)\n"
              .. "4. Concurrency issues (sync/async bridges, lock contention, race conditions)\n"
              .. "5. Test coverage gaps\n"
              .. "6. Code duplication or abstraction leaks\n"
              .. "7. Performance concerns (blocking calls in async context, unnecessary allocations)\n\n"
              .. "Also note strengths: good patterns, solid tests, clean abstractions.",
            schema = ANALYSIS_SCHEMA,
            name = "analyze-" .. sub.name,
            description = "Analyze " .. sub.name .. " for issues",
          })
          if not r.ok then
            log("analyze failed for " .. sub.name .. ": " .. (r.status or "unknown"), "warn")
            return { ok = false, output = { subsystem = sub.name, issues = {}, strengths = {} } }
          end
          return r
        end,
      },
      {
        label = "assess",
        handler = function(prev)
          local sub_name = "unknown"
          if prev and prev.output then
            sub_name = prev.output.subsystem or "unknown"
          end
          if not prev.ok then
            log("degrading assess for " .. sub_name, "warn")
            return {
              ok = true,
              output = {
                subsystem = sub_name,
                health_score = 0,
                top_risks = { "analysis failed - manual review needed" },
                recommended_actions = {
                  { action = "Manually review " .. sub_name, priority = "P0", impact = "unknown state" },
                },
              },
            }
          end
          phase("assess " .. sub_name)
          local r = agent({
            prompt = "Based on this analysis, assess the health of subsystem \"" .. sub_name .. "\".\n\n"
              .. "Analysis:\n" .. json.encode(prev.output) .. "\n\n"
              .. "Provide:\n"
              .. "1. health_score (0-100): overall code health\n"
              .. "2. top_risks: up to 5 most critical risks\n"
              .. "3. recommended_actions: prioritized list with P0-P3 priority, each with impact description\n\n"
              .. "Prioritize by: security > correctness > reliability > performance > maintainability",
            schema = ASSESS_SCHEMA,
            name = "assess-" .. sub_name,
            description = "Assess " .. sub_name .. " health",
          })
          if not r.ok then
            log("assess failed for " .. sub_name, "warn")
            return {
              ok = false,
              output = {
                subsystem = sub_name,
                health_score = 0,
                top_risks = {},
                recommended_actions = {},
              },
            }
          end
          return r
        end,
      },
    },
  }

  local assessments = {}
  for i, item in ipairs(results.items or {}) do
    if item.output then
      table.insert(assessments, item.output)
    end
  end

  phase("triage", 1)
  local t = agent({
    prompt = "You are the triage lead for the Maestro project codebase health audit.\n\n"
      .. "Below are health assessments for each subsystem. Synthesize them into a prioritized improvement plan.\n\n"
      .. json.encode(assessments) .. "\n\n"
      .. "Provide:\n"
      .. "1. overall_health: weighted average health score (0-100)\n"
      .. "2. executive_summary: 2-3 sentence overview of project health\n"
      .. "3. priority_matrix: categorize actions into quadrants:\n"
      .. "   - Quick Wins (high impact, low effort)\n"
      .. "   - Strategic (high impact, high effort)\n"
      .. "   - Fill-ins (low impact, low effort)\n"
      .. "   - Thankless (low impact, high effort)\n"
      .. "4. quick_wins: top 5 actions to take immediately\n"
      .. "5. strategic_initiatives: 2-3 larger efforts for the roadmap",
    schema = TRIAGE_SCHEMA,
    name = "triage",
    description = "Cross-cutting triage and prioritization",
  })
  if not t.ok then
    report({ error = "triage failed: " .. (t.status or "unknown") })
    return
  end

  phase("report")
  report({
    overall_health = t.output.overall_health,
    executive_summary = t.output.executive_summary,
    subsystems_assessed = #assessments,
    priority_matrix = t.output.priority_matrix,
    quick_wins = t.output.quick_wins,
    strategic_initiatives = t.output.strategic_initiatives or {},
    details = assessments,
  })
end
