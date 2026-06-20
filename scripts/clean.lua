budget(180000, 20)

phase("tui-ws-removal", 4)
log("Removing TUI and WebSocket implementations", "info")

local items = { "delete", "edit_source", "edit_config" }
local results = parallel(items, function(item)
  if item == "delete" then
    return {
      prompt = [[You are working on the Maestro project at /Users/apple/dev/maestro.
Delete ALL WebSocket and TUI-related files:

1. rm -rf src/ws/
2. rm -f src/commands/serve.rs
3. rm -f docs/design/tui.md docs/design/tui-interaction.md docs/design/websocket-server.md docs/design/ws-test.md docs/design/web-ui.md
4. Verify each path is gone (ls or test -f).

Return JSON { files_deleted: [string] }]],
      schema = {
        type = "object",
        properties = { files_deleted = { type = "array", items = { type = "string" } } },
        required = { "files_deleted" }
      }
    }
  elseif item == "edit_source" then
    return {
      prompt = [[You are working on the Maestro project at /Users/apple/dev/maestro.
Edit source files to remove all WebSocket and TUI references.

Modify these files (read each first, then edit):

1. src/lib.rs: Remove the line "pub mod ws;"

2. src/main.rs:
   - Remove the doc comment line about --headless (line 7)
   - Remove the Commands::Serve variant entirely
   - Remove the --headless field from RunArgs (lines 103-104)
   - Always default log level to "warn" (remove the Serve match)
   - Remove log_file handling entirely (always None); remove the log_file variable from the match
   - Remove the Commands::Serve dispatch arm (lines 163-165)
   - Change logging::init call to: logging::init(cli.log_level.as_deref(), "warn")?;
   - Remove the _log_guard variable (init returns () now)

3. src/commands/mod.rs: Remove "pub mod serve;"

4. src/commands/run.rs:
   - Update module doc: remove "TUI / headless" reference
   - Remove the if args.headless branching — always call run_headless directly
   - Remove the run_tui function entirely
   - Update comments that reference TUI (lines 80, 97, 178-180 context)

5. src/service/run.rs:
   - Update comment on lines 178-180: remove "the WS layer stores them in its RunHandle"
   - Update line 253: remove "both the CLI and WS"

6. src/logging.rs:
   - Remove tracing_appender import
   - Remove the `file: Option<&Path>` parameter from init()
   - Remove all file-logging code and tracing_appender references
   - Change return type to anyhow::Result<()>
   - Simplify to just stderr layer with filter

Return JSON { modified_files: [string], summary: string }]],
      schema = {
        type = "object",
        properties = { modified_files = { type = "array", items = { type = "string" } }, summary = { type = "string" } },
        required = { "modified_files", "summary" }
      }
    }
  else
    return {
      prompt = [[You are working on the Maestro project at /Users/apple/dev/maestro.
Edit Cargo.toml and documentation files.

1. Cargo.toml: Remove these three dependency lines:
   - axum = { version = "0.7", features = ["ws"] }   (only used by ws module)
   - tokio-stream = { version = "0.1", features = ["sync"] }   (only used by ws module)
   - tracing-appender = "0.2"   (only used by serve's --log-file)

2. docs/architecture/cli.md:
   - Section 1 diagram: remove the TUI branch, keep only headless
   - Section 4: remove the TUI row from the output mode table
   - Section 7: remove the TUI bullet (line 111)
   - Any other TUI references

3. docs/architecture.md:
   - Module index table: change "TUI/headless 输出" to "headless 输出"
   - Line 62: remove "TUI（[cli](./architecture/cli.md)）" from the event bus description
   - Line 128: remove the "TUI 为文本桩" bullet

Return JSON { modified_files: [string], summary: string }]],
      schema = {
        type = "object",
        properties = { modified_files = { type = "array", items = { type = "string" } }, summary = { type = "string" } },
        required = { "modified_files", "summary" }
      }
    }
  end
end)

local del_result = results[1]
local edit_source_result = results[2]
local edit_config_result = results[3]

if not del_result.ok then
  report({ error = "deletion failed: " .. del_result.status })
end
if not edit_source_result.ok then
  report({ error = "source edit failed: " .. edit_source_result.status })
end
if not edit_config_result.ok then
  report({ error = "config edit failed: " .. edit_config_result.status })
end

phase("verify", 1)
log("Running cargo check to verify compilation", "info")

local verify = agent({
  prompt = [[Run `cargo check 2>&1` in /Users/apple/dev/maestro.

Return JSON { ok: bool, output: string }.]],
  schema = {
    type = "object",
    properties = { ok = { type = "boolean" }, output = { type = "string" } },
    required = { "ok", "output" }
  }
})

if not verify.ok then
  report({ error = "verification failed: " .. verify.status })
end

local all_modified = {}
if edit_source_result.ok then
  for _, f in ipairs(edit_source_result.output.modified_files or {}) do
    table.insert(all_modified, f)
  end
end
if edit_config_result.ok then
  for _, f in ipairs(edit_config_result.output.modified_files or {}) do
    table.insert(all_modified, f)
  end
end

report({
  status = "completed",
  summary = "Removed TUI and WebSocket implementations",
  files_deleted = del_result.ok and del_result.output.files_deleted or {},
  files_modified = all_modified,
  compilation_ok = verify.ok and verify.output.ok or false,
  compilation_output = verify.ok and verify.output.output or "N/A",
})