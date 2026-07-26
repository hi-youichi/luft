# Agent Prompt Quality

The orchestration script's quality is bounded by the prompts it sends to agents.
A vague prompt produces a vague result; no schema can compensate for missing
context. Follow these principles:

1. Include concrete context. The agent has tools but needs to know WHERE to
   look. Pass file paths, module names, search queries — not just an action verb.
2. Be specific about what to find or produce. List the exact criteria.
3. When using a schema (analysis agents), align prompt with schema. The prompt
   defines WHAT to extract; the schema defines the STRUCTURE. They must match.
4. For file-writing tasks, tell the agent which tool to use and the exact path
   (see Rule 11).

BAD (vague — agent will guess, results will be useless):
```lua
prompt = "Analyze " .. mod .. " for issues"
```

GOOD (specific — agent knows what to look at and what to return):
```lua
prompt = "You are reviewing the Rust module at `" .. mod.path .. "`.\n"
      .. "Read the source files and identify:\n"
      .. "1. Functions exceeding 50 lines that should be split\n"
      .. "2. Duplicate logic across files\n"
      .. "3. Missing error handling on fallible calls\n\n"
      .. "For each issue, provide: file path, line range, and a concrete fix.\n"
      .. "Return the results matching the schema."
```
