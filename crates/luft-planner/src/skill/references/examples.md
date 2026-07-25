# Example: per-module refactoring (static decomposition)
```lua
meta = {
  reasoning = "Decompose by module; analyze, refactor, verify each in sequence",
  phases = {
    { label = "analyze", description = "Analyze each module for issues",
      agents = 3 },
    { label = "refactor", description = "Apply refactoring to each module",
      agents = 3 },
    { label = "verify", description = "Verify refactored modules pass tests",
      agents = 3 },
    { label = "report" },
  },
}

local MODULES = { "auth", "db", "api" }

local ANALYSIS = {
  type = "object",
  properties = {
    issues = { type = "array", items = { type = "string" } },
    summary = { type = "string" }
  },
  required = { "issues", "summary" }
}
local CHANGES = {
  type = "object",
  properties = {
    changed = { type = "boolean" },
    files_modified = { type = "array", items = { type = "string" } }
  },
  required = { "changed" }
}
local VERIFY = {
  type = "object",
  properties = {
    passed = { type = "boolean" },
    details = { type = "string" }
  },
  required = { "passed" }
}

function main()
  local results = {}

  for _, mod in ipairs(MODULES) do
    local name = "refactor " .. mod
    phase(name)
    phase("analyze")
    local a = agent({
      prompt = "You are reviewing the module `" .. mod .. "` under src/.\n"
            .. "Read the source files and identify:\n"
            .. "1. Functions exceeding 50 lines that should be split\n"
            .. "2. Duplicate logic across files\n"
            .. "3. Missing error handling on fallible calls\n\n"
            .. "For each issue: file path, line range, concrete fix.",
      schema = ANALYSIS
    })
    if not a.ok then
      log("analyze failed for " .. mod, "warn")
      goto continue
    end

    phase("refactor")
    local c = agent({
      prompt = "Apply the following refactoring changes to module `" .. mod .. "`:\n"
            .. json.encode(a.output.issues)
            .. "\nUse the str_replace_based_edit_tool to edit each file.",
      schema = CHANGES
    })
    if not c.ok then
      log("refactor failed for " .. mod, "warn")
      goto continue
    end

    phase("verify")
    local v = agent({
      prompt = "Verify module `" .. mod .. "` still passes tests after refactoring.\n"
            .. "Run `cargo test` and report pass/fail with details.",
      schema = VERIFY
    })
    table.insert(results, { module = mod, ok = v.ok and v.output.passed })
    ::continue::
  end

  report({ refactored = #results, results = results })
end
```

# Example: whole-crate refactoring (dynamic enumeration, nested loops)
```lua
--------------------------------------------
-- Goal:  Refactor entire crate by subsystem
-- Arch:
--   discover ==> [subsystems[]]
--     (for each subsystem)
--       discover ==> [modules[]]
--         (for each module)
--           analyze ==> change ==> verify --> [result]
--   <== report
-- Flow:  discover -> subsystems[] -> modules[] -> changes -> report
--------------------------------------------
meta = {
  reasoning = "Two-stage discovery: enumerate subsystems, then modules per subsystem",
  phases = {
    { label = "discover subsystems", description = "Enumerate subsystems needing refactoring" },
    { label = "discover modules", description = "Enumerate modules per subsystem",
      dynamic = true },
    { label = "analyze", description = "Analyze each module for issues",
      dynamic = true },
    { label = "change", description = "Apply changes to each module",
      dynamic = true },
    { label = "verify", description = "Verify each module passes tests",
      dynamic = true },
    { label = "report" },
  },
}

local SUBSYSTEMS_SCHEMA = {
  type = "object",
  properties = {
    subsystems = {
      type = "array",
      items = {
        type = "object",
        properties = {
          name = { type = "string" },
          path = { type = "string" }
        },
        required = { "name", "path" }
      }
    }
  },
  required = { "subsystems" }
}
local MODULES_SCHEMA = {
  type = "object",
  properties = {
    modules = {
      type = "array",
      items = {
        type = "object",
        properties = {
          name = { type = "string" },
          path = { type = "string" }
        },
        required = { "name", "path" }
      }
    }
  },
  required = { "modules" }
}

function main()
  phase("discover subsystems")
  local discover = agent({
    prompt = "Scan the crate under src/ and list subsystems (top-level directories "
          .. "or module groups) that need refactoring. For each, give name and path.",
    schema = SUBSYSTEMS_SCHEMA
  })
  if not discover.ok then
    report({ error = "discovery failed: " .. discover.status })
    return
  end

  local results = {}

  for _, sys in ipairs(discover.output.subsystems or {}) do
    local gname = "refactor " .. sys.name
    phase(gname)
    local mods = agent({
      prompt = "List modules in `" .. sys.path .. "` that need changes.\n"
            .. "Give name and path for each.",
      schema = MODULES_SCHEMA
    })
    if not mods.ok then
      log("module discovery failed for " .. sys.name, "warn")
      goto next_subsystem
    end

    for _, mod in ipairs(mods.output.modules or {}) do
      local mname = "refactor " .. mod.name
      phase(mname)
      phase("analyze")
      local a = agent({
        prompt = "Analyze `" .. mod.path .. "` for refactoring opportunities:\n"
              .. "long functions, duplication, missing error handling. "
              .. "Return a summary.",
        schema = { type = "object", properties = { summary = { type = "string" } },
                   required = { "summary" } }
      })
      if not a.ok then
        log("analyze failed for " .. mod.name, "warn")
        goto next_module
      end
      phase("change")
      local c = agent({
        prompt = "Apply refactoring to `" .. mod.path .. "` based on:\n"
              .. a.output.summary
              .. "\nUse str_replace_based_edit_tool. Report whether changes were made.",
        schema = { type = "object", properties = { changed = { type = "boolean" } },
                   required = { "changed" } }
      })
      if not c.ok then
        log("change failed for " .. mod.name, "warn")
        goto next_module
      end
      phase("verify")
      local v = agent({
        prompt = "Verify `" .. mod.path .. "` passes tests after changes.\n"
              .. "Run `cargo test` and report pass/fail.",
        schema = { type = "object", properties = { passed = { type = "boolean" } },
                   required = { "passed" } }
      })
      table.insert(results, {
        module = mod.name,
        changed = c.output.changed,
        passed = v.ok and v.output.passed or false
      })
      ::next_module::
    end
    ::next_subsystem::
  end

  report({ modules_refactored = #results, results = results })
end
```

# Example: adversarial verification (cross-check findings via voting)
```lua
meta = {
  reasoning = "Multi-round adversarial loop: vote on each finding, keep approved, iterate",
  phases = {
    { label = "gather", description = "Initial findings to cross-check" },
    { label = "vote", description = "Adversarial voting rounds",
      dynamic = true },
    { label = "report" },
  },
}

local FINDINGS_SCHEMA = {
  type = "object",
  properties = {
    findings = {
      type = "array",
      items = {
        type = "object",
        properties = {
          claim = { type = "string" },
          evidence = { type = "string" }
        },
        required = { "claim" }
      }
    }
  },
  required = { "findings" }
}

local VOTE_SCHEMA = {
  type = "object",
  properties = { approve = { type = "boolean" }, reason = { type = "string" } },
  required = { "approve" }
}

function main()
  phase("gather", 1)
  local gather = agent({
    prompt = "List key findings to verify. For each finding, state the claim "
          .. "and supporting evidence.",
    schema = FINDINGS_SCHEMA
  })
  if not gather.ok then
    report({ error = "gather failed" })
    return
  end

  local items = gather.output.findings or {}
  local max_rounds = 3
  local threshold_rate = 0.7
  local voters_per_item = 3

  for round = 1, max_rounds do
    phase("vote round " .. round)
    log("adversarial round " .. round .. ", " .. #items .. " items")

    local vote_tasks = {}
    for i, finding in ipairs(items) do
      for v = 1, voters_per_item do
        table.insert(vote_tasks, { item_idx = i, finding = finding })
      end
    end

    local all_votes = parallel(vote_tasks, function(task)
      return {
        prompt = "Evaluate this finding for accuracy and completeness.\n"
              .. json.encode(task.finding)
              .. "\nVote approve=true only if the claim is well-supported.",
        schema = VOTE_SCHEMA
      }
    end)

    local survivors = {}
    for i, finding in ipairs(items) do
      local approved = 0
      for j = 1, voters_per_item do
        local v = all_votes[(i - 1) * voters_per_item + j]
        if v.ok and v.output.approve then approved = approved + 1 end
      end
      if approved / voters_per_item >= threshold_rate then
        table.insert(survivors, finding)
      end
    end

    if #survivors == #items then
      log("converged after round " .. round)
      break
    end
    items = survivors
  end

  report({ survivors = #items, findings = items })
end
```
