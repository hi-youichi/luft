# Primitives (available as Lua globals)

## agent(opts) -> result
    Runs ONE subagent to completion. This is the fundamental work unit.
    opts:   { prompt=<string, required>,     -- instructions for the subagent (see references/agent-prompts.md)
              schema=<table?>,               -- JSON Schema to constrain output (see Rule 6)
              model=<string?>,               -- override model (default: backend's model)
              name=<string?>,                -- short agent identifier shown in CLI (e.g. "analyze-auth")
              description=<string?>,         -- one-line description shown in CLI
              timeout_ms=<int?> }            -- per-agent timeout
    result: { ok=<bool>,                     -- true if agent succeeded
              status=<string>,               -- "ok" / "error" / "cancelled" / "timed_out"
              output=<table>,                -- agent response (parsed JSON -> Lua table)
              tokens=<int>,                  -- token usage
              findings=<array> }             -- accumulated findings (if any)

    When generating with --with-mock, EVERY agent() call MUST include a unique
    `name=` field. This name matches mock responses in the .mock.json sidecar.
    For parallel() fan-out, all items share one name; the mock response is reused.

    schema — see Rule 6 for when to use / skip. Example:
      Define named schema tables at the top of the script and reuse them:
        local FINDINGS = {
          type = "object",
          properties = {
            files = { type = "array",
                      items = { type = "object",
                                properties = { path = { type = "string" },
                                               purpose = { type = "string" } },
                                required = { "path", "purpose" } } },
            summary = { type = "string" }
          },
          required = { "files", "summary" }
        }
      Then: agent({ prompt = "...", schema = FINDINGS })

## parallel(items, mapFn) -> array<result>
    Barrier fan-out: runs all items concurrently, waits for ALL to finish.
    items:  array of work items (any Lua table).
    mapFn:  function(item) -> must RETURN an agent opts table.
    Result: array of agent results, preserving input order.
    Use when: you need ALL results before continuing (e.g. gather -> analyze all).

    Example:
      local results = parallel(urls, function(url)
        return { prompt = "Fetch and summarize: " .. url, schema = SUMMARY }
      end)

## pipeline{ items=, stages=, max_inflight= } -> { items=, ok=, failed= }
    Streaming multi-stage: each item flows through all stages; different items can
    be in different stages simultaneously. Prefer pipeline() over parallel() by default.

    IMPORTANT: Unlike parallel(), pipeline stage handlers are NOT auto-executed. Each
    handler MUST call agent() itself and return the result (or custom data). The return
    value becomes the input to the next stage.

    Parameters:
      items:       array of work items.
      stages:      array of stages. Each stage is either a function(prev) or a
                   table { label=, handler=function(prev) }. The handler receives
                   the previous stage's return value (or the raw item for stage 1),
                   calls agent() internally, and returns its result.
      max_inflight: max concurrent items (default: 4).

    Stage data flow:
      Stage 1:  handler(item)     -> [calls agent()] -> return value(data1)
      Stage 2:  handler(data1)    -> [calls agent()] -> return value(data2)
      ...
      pipeline_result.items[i].output is the LAST stage's return value for item i.

    Error degradation:
      If a stage returns a failed result (prev.ok = false), the next stage still
      receives it. Check `prev.ok` at the start of each handler and decide: degrade
      gracefully or abort. On degrade, return default data directly (do NOT call agent).

    Example (2-stage: analyze -> assess):
      local results = pipeline{
        items = modules,
        max_inflight = 4,
        stages = {
          function(mod)
            phase("analyze " .. mod.name)
            return agent({ prompt = "Analyze " .. mod.path, schema = ANALYSIS })
          end,
          function(prev)
            phase("assess " .. (prev.output and prev.output.module or "?"))
            if not prev.ok then
              return { ok = false, output = { module = "unknown", score = 0 } }
            end
            return agent({ prompt = "Assess: " .. json.encode(prev.output), schema = ASSESS })
          end
        }
      }

## phase(name, planned?) -> phase_id
    Declares a progress phase. Emits a PhaseStarted event visible in CLI output.
    name:    human-readable label (shown in CLI phase tree).
    planned: expected agent count (optional, for progress display).

## log(msg, level?)
    Emits a status line visible in CLI output and event log.
    level: "info" (default) / "warn" / "error".

## budget(time_ms?, max_rounds?)
    Hints resource limits for the current phase. Optional.
    Example:
      budget(60000, 5)  -- 60s or 5 rounds, whichever comes first

## workflow(path, args?) -> result
    Calls another saved workflow as a sub-step.
    path: relative path to the .lua workflow file.
    args: table of arguments passed to the sub-workflow.

## report(value)
    REQUIRED: sets the final workflow output and ends the run.
    Call exactly ONCE — the first call wins; later calls are ignored.
    Always `return` after an error report() to prevent fall-through.

## json.encode(value) / json.decode(string)
    JSON serialization helpers for passing structured data to/from agent prompts.
