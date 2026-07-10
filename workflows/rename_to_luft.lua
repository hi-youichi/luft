-- rename_to_luft.lua
-- 用 Luft 自身的工作流系统，把整个 luft 项目改名为 luft。
--
-- 替换规则（大小写敏感，按特异性从高到低）：
--   luft_core     → luft_core        (Rust crate name in use statements)
--   luft_storage  → luft_storage
--   luft_runtime  → luft_runtime
--   luft_adapters → luft_adapters
--   luft_planner  → luft_planner
--   luft_service  → luft_service
--   luft_mcp      → luft_mcp
--   luft-core     → luft-core        (Cargo.toml package names & paths)
--   luft-cli      → luft-cli
--   LuftBuilder   → LuftBuilder      (struct / type names)
--   LuftError     → LuftError
--   Luft          → Luft             (main facade struct)
--   .luft/        → .luft/           (runtime dirs)
--   hi-youichi/luft → hi-youichi/luft (repo URL)
--   name = "luft" → name = "luft"    (binary name)
--   "luft"        → "luft"           (remaining string literals)

meta = {
    phases = {
        {
            label = "dirs",
            detail = "Rename 9 crate directories from luft-* to luft-*",
            agents = 1,
            depends_on = {}
        },
        {
            label = "content",
            detail = "Batch find-replace luft→luft in all source/config/doc files, grouped by crate",
            agents = 8,
            depends_on = { 0 }
        },
        {
            label = "verify",
            detail = "Run cargo check and report any remaining 'luft' references",
            agents = 1,
            depends_on = { 1 }
        }
    },
    reasoning = "Three-phase rename: first move directories (so paths are stable for subsequent edits), then fan-out parallel agents to mechanically replace all 'luft' tokens in source/config/docs, finally verify the build compiles and no stray references remain."
}

function main()
    budget(300000, 12)

    -- ── Phase 1: Rename directories ──────────────────────────────
    phase("dirs", 1)

    local dirs_result = agent({
        prompt = "You are performing a project rename from 'luft' to 'luft' in the Rust workspace at C:\\Users\\heycj\\dev\\luft.\n\n"
            .. "TASK: Rename the following 9 crate directories under crates/ using your file-move/rename capability:\n\n"
            .. "  crates/luft-core     → crates/luft-core\n"
            .. "  crates/luft-storage  → crates/luft-storage\n"
            .. "  crates/luft-runtime  → crates/luft-runtime\n"
            .. "  crates/luft-adapters → crates/luft-adapters\n"
            .. "  crates/luft-planner  → crates/luft-planner\n"
            .. "  crates/luft-service  → crates/luft-service\n"
            .. "  crates/luft          → crates/luft\n"
            .. "  crates/luft-mcp      → crates/luft-mcp\n"
            .. "  crates/luft-cli      → crates/luft-cli\n\n"
            .. "IMPORTANT:\n"
            .. "- Rename ALL 9 directories. Do not skip any.\n"
            .. "- Use PowerShell or your move/rename tool. On Windows use 'Move-Item' or 'Rename-Item'.\n"
            .. "- Do NOT touch the target/ directory.\n"
            .. "- Do NOT touch the .luft/ directory (runtime state).\n\n"
            .. "Return a JSON object: { 'renamed': ['luft-core', 'luft-storage', ...], 'errors': [] }"
    })

    local renamed = dirs_result.output.renamed or {}
    log("Phase 1 complete: renamed " .. #renamed .. " directories")

    -- ── Phase 2: Parallel content replacement ────────────────────
    phase("content", 8)

    -- Define the replacement rules as a single block that every agent receives
    local rules = [[
REPLACEMENT RULES (apply EXACTLY, case-sensitive, in this order):
  1. luft_core     → luft_core
  2. luft_storage  → luft_storage
  3. luft_runtime  → luft_runtime
  4. luft_adapters → luft_adapters
  5. luft_planner  → luft_planner
  6. luft_service  → luft_service
  7. luft_mcp      → luft_mcp
  8. luft-core     → luft-core
  9. luft-cli      → luft-cli
  10. LuftBuilder  → LuftBuilder
  11. LuftError    → LuftError
  12. Luft         → Luft
  13. .luft/       → .luft/
  14. hi-youichi/luft → hi-youichi/luft
  15. "luft"       → "luft"
  16. 'luft'       → 'luft'
  17. name = "luft" → name = "luft"

IMPORTANT NOTES:
- Apply rules in order (most specific first) to avoid partial replacements.
- Rule 12 (Luft→Luft) is a SUPERSET — only apply AFTER rules 10-11 to avoid LuftBuilder getting double-replaced.
- Actually, since we go specific→general: first replace LuftBuilder→LuftBuilder and LuftError→LuftError, THEN replace remaining Luft→Luft. This is correct.
- For rules 13-16, these are for string literals, paths, and doc comments.
- Do NOT modify any file under target/ or .luft/.
- Use your Read tool to read each file, then use Edit/Write to apply ALL applicable replacements.
- Be thorough: a file may need multiple different rules applied.
]]

    -- Group tasks for parallel execution
    local tasks = {
        {
            name = "root-cargo",
            detail = "Root Cargo.toml (workspace members, repository URL) and Cargo.lock",
            files = "Cargo.toml, Cargo.lock"
        },
        {
            name = "luft-core",
            detail = "crates/luft-core/ — Cargo.toml + all .rs files",
            files = "crates/luft-core/Cargo.toml and all crates/luft-core/src/**/*.rs"
        },
        {
            name = "luft-runtime",
            detail = "crates/luft-runtime/ — Cargo.toml + all .rs files",
            files = "crates/luft-runtime/Cargo.toml and all crates/luft-runtime/src/**/*.rs"
        },
        {
            name = "luft-storage",
            detail = "crates/luft-storage/ — Cargo.toml + all .rs files",
            files = "crates/luft-storage/Cargo.toml and all crates/luft-storage/src/**/*.rs"
        },
        {
            name = "luft-adapters",
            detail = "crates/luft-adapters/ — Cargo.toml + all .rs files",
            files = "crates/luft-adapters/Cargo.toml and all crates/luft-adapters/src/**/*.rs"
        },
        {
            name = "luft-planner+service",
            detail = "crates/luft-planner/ and crates/luft-service/ — Cargo.toml + all .rs files",
            files = "crates/luft-planner/Cargo.toml, crates/luft-planner/src/**/*.rs, crates/luft-service/Cargo.toml, crates/luft-service/src/**/*.rs"
        },
        {
            name = "luft+mcp+cli",
            detail = "crates/luft/ and crates/luft-mcp/ and crates/luft-cli/ — Cargo.toml + all .rs files",
            files = "All .toml and .rs files under crates/luft/, crates/luft-mcp/, crates/luft-cli/"
        },
        {
            name = "docs-configs",
            detail = "README.md, CHANGELOG.md, CONTRIBUTING.md, .gitignore, workflows/*.lua, examples/*.lua",
            files = "README.md, CHANGELOG.md, CONTRIBUTING.md, .gitignore, all *.lua under workflows/ and examples/"
        },
    }

    local results = parallel(tasks, function(task)
        return {
            prompt = "You are performing a mechanical project rename from 'luft' to 'luft' "
                .. "in the Rust workspace at C:\\Users\\heycj\\dev\\luft (now being renamed to luft).\n\n"
                .. "YOUR TASK GROUP: " .. task.name .. "\n"
                .. "Files to process: " .. task.files .. "\n\n"
                .. rules .. "\n\n"
                .. "INSTRUCTIONS:\n"
                .. "1. Read each file in your task group using your Read tool.\n"
                .. "2. Apply ALL applicable replacement rules to each file using Edit (or Write for full rewrites).\n"
                .. "3. After editing, verify no 'luft' (case-insensitive) remains in the files you edited.\n"
                .. "4. Be meticulous — missing even one reference will break the build.\n\n"
                .. "Return JSON: { 'task': '" .. task.name .. "', 'files_modified': <count>, 'total_replacements': <count>, 'errors': [] }"
        }
    end)

    -- Collect stats
    local total_files = 0
    local total_replacements = 0
    local all_errors = {}
    for _, r in ipairs(results) do
        if r.ok and r.output then
            total_files = total_files + (r.output.files_modified or 0)
            total_replacements = total_replacements + (r.output.total_replacements or 0)
            if r.output.errors then
                for _, e in ipairs(r.output.errors) do
                    table.insert(all_errors, e)
                end
            end
        end
    end
    log("Phase 2 complete: " .. total_files .. " files modified, " .. total_replacements .. " replacements, " .. #all_errors .. " errors")

    -- ── Phase 3: Verify ──────────────────────────────────────────
    phase("verify", 1)

    local verify_result = agent({
        prompt = "You are the verification agent for a project rename from 'luft' to 'luft' "
            .. "in the Rust workspace at C:\\Users\\heycj\\dev\\luft.\n\n"
            .. "TASK:\n"
            .. "1. Search for any remaining 'luft' references (case-insensitive) in all files under crates/, workflows/, examples/, "
            .. "and root-level files (Cargo.toml, README.md, etc). EXCLUDE: target/, .luft/, .git/, Cargo.lock (lockfile may still reference old published versions).\n"
            .. "2. Fix any remaining references you find using the replacement rules.\n"
            .. "3. Run 'cargo check' in the workspace root to verify the project compiles.\n"
            .. "4. If cargo check fails, read the errors and fix the root cause (likely missed replacements).\n\n"
            .. "Return JSON: {\n"
            .. "  'remaining_references': [{file, line, text}],\n"
            .. "  'fixed_during_verify': <count>,\n"
            .. "  'cargo_check_passed': true/false,\n"
            .. "  'cargo_errors': [string],\n"
            .. "  'summary': 'one-line summary'\n"
            .. "}"
    })

    -- ── Final report ─────────────────────────────────────────────
    report({
        workflow = "rename_to_luft",
        project = "luft → luft rename",
        directories_renamed = #renamed,
        files_modified = total_files,
        total_replacements = total_replacements,
        phase2_errors = all_errors,
        verification = verify_result.output,
    })
end
