phase("explore", 3)

local listing = agent({
  prompt = [[You are a codebase architect. Explore this repository and produce a comprehensive map of all TUI-related and WebSocket-related code.

1. Use glob + grep to find all files mentioning TUI (tui, terminal UI, bubbletea, bubble_tea, tea, term, terminal, views, components, etc.)
2. Use glob + grep to find all files mentioning WebSocket (websocket, ws, socket, upgrade, wsconn, etc.)
3. For each area, identify:
   - Entry points / startup paths
   - Key types, structs, interfaces
   - The relationship between TUI and WebSocket (does the TUI connect via WS? does WS push to TUI?)
   - Any dead code, commented-out code, deprecated paths
   - Any duplicated or overlapping types/utilities
   - Testing coverage

Return a JSON object with:
{
  "tui": { "files": [{"path": "...", "role": "...", "lines": int, "issues": ["..."]}], "summary": "...", "dead_code": ["..."] },
  "websocket": { "files": [...], "summary": "...", "dead_code": ["..."] },
  "integration_points": ["..."],
  "overlapping_or_duplicated": ["..."]
}
Do NOT write any files — just return the JSON. Be thorough: scan the entire codebase.]]
})

local tui = listing.output.tui or {}
local ws = listing.output.websocket or {}
local integration = listing.output.integration_points or {}
local overlaps = listing.output.overlapping_or_duplicated or {}

phase("analyze", 2)

local analysis = agent({
  prompt = [[You are a codebase cleanup specialist. Based on the following mapping of TUI and WebSocket code, produce a concrete cleanup plan.

TUI data: ]] .. json.encode(tui) .. [[

WebSocket data: ]] .. json.encode(ws) .. [[

Integration points: ]] .. json.encode(integration) .. [[

Overlaps/duplication: ]] .. json.encode(overlaps) .. [[

Analyze and return a JSON object with:
{
  "cleanup_plan": [
    {
      "id": "CLEANUP-1",
      "area": "tui"|"websocket"|"integration",
      "title": "...",
      "description": "...",
      "rationale": "...",
      "actions": [{"file": "...", "action": "delete"|"refactor"|"merge"|"move"|"extract"|"rewrite", "detail": "..."}],
      "risk": "low"|"medium"|"high",
      "effort": "small"|"medium"|"large"
    }
  ],
  "prioritization": "Which items to do first and why",
  "risks": ["Risk 1", "Risk 2"],
  "suggested_phases": [
    {"phase": 1, "items": ["CLEANUP-1", ...], "goal": "..."}
  ]
}
Be specific with file paths and concrete code suggestions. Drive out duplication, dead code, and architectural inconsistencies.]]
})

phase("detail", 1)

local detail = agent({
  prompt = [[You are a senior engineer doing code cleanup. Here is a cleanup plan:

]] .. json.encode(analysis.output) .. [[

Pick the HIGHEST priority cleanup items (at most 3) and produce the actual code-level diffs/changes needed.

For each selected item, return:
{
  "cleanups": [
    {
      "id": "CLEANUP-1",
      "files_to_modify": [
        {
          "path": "...",
          "changes": [
            {"type": "edit", "old_string": "...existing code...", "new_string": "...replacement..."},
            {"type": "delete_file"},
            {"type": "create_file", "content": "..."}
          ]
        }
      ],
      "verification": ["What to check after applying"]
    }
  ],
  "skipped": ["Items not detailed and why"]
}

Be precise — return runnable edit operations. Do NOT write files yourself.]]
})

report({
  summary = "TUI and WebSocket cleanup analysis",
  tui_summary = tui.summary,
  websocket_summary = ws.summary,
  integration_points = integration,
  overlaps = overlaps,
  cleanup_plan = analysis.output,
  detailed_cleanups = detail.output,
  note = "The cleanup_plan and detailed_cleanups contain the actionable items. "
       .. "Apply detailed_cleanups.cleanups first, then iterate through cleanup_plan items."
})