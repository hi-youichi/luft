meta = {
    phases = {
        {
            label = "smell_hunt",
            description = "One agent per file hunts for code smells (Bloaters, OO-Abusers, Change-Preventers, Dispensables, Couplers, Rust-specific). All 5 agents run in parallel.",
            agents = 5,
            depends_on = {}
        },
        {
            label = "cross_reference",
            description = "Single agent examines all 5 per-file smell reports to find cross-file smells that only become visible when comparing files side-by-side (duplication, inconsistency, scattered concerns, missing abstractions).",
            agents = 1,
            depends_on = { 0 }
        },
        {
            label = "synthesize",
            description = "Single lead-reviewer agent aggregates per-file smells + cross-reference findings into a prioritized, actionable final report with top-10 smells, per-file scores, and refactor recommendations.",
            agents = 1,
            depends_on = { 1 }
        }
    },
    reasoning = "Three-stage pipeline. First, fan out one agent per file in src/adapters/ to hunt for code smells in isolation (intra-file analysis). Then run a single cross-reference agent to find smells that only become visible when comparing files side-by-side (duplicated logic, inconsistent error handling, scattered concerns). Finally, synthesize all findings into a prioritized, actionable report. The cross-reference stage is essential because many smells (shotgun surgery, parallel inheritance, divergent change) cannot be detected from a single file in isolation."
}

local ADAPTERS_FILES = {
    { path = "src/adapters/mod.rs",              kind = "facade",      role = "Public re-exports + register_acp_backend wiring" },
    { path = "src/adapters/acp_adapter.rs",      kind = "primary",     role = "AcpAdapter struct + ACP client subprocess lifecycle" },
    { path = "src/adapters/permission.rs",       kind = "policy",      role = "Non-interactive request_permission decision logic" },
    { path = "src/adapters/result_collector.rs", kind = "collector",   role = "ACP stop_reason + message -> AgentResult aggregation" },
    { path = "src/adapters/update_mapper.rs",    kind = "translation", role = "ACP SessionUpdate -> Luft ProgressDelta mapping" }
}

local FILE_SUMMARY = {
    type = "object",
    properties = {
        file         = { type = "string" },
        kind         = { type = "string" },
        summary      = { type = "string" },
        line_count   = { type = "integer" },
        public_items = { type = "integer" },
        smells = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    category    = { type = "string", description = "Bloaters | OO-Abusers | Change-Preventers | Dispensables | Couplers | Rust-Specific" },
                    name        = { type = "string", description = "Specific smell name, e.g. 'Long Method', 'Unwrap Abuse'" },
                    severity    = { type = "string", description = "high | medium | low" },
                    location    = { type = "string", description = "fn name + line hint, e.g. 'AcpAdapter::run, L420'" },
                    description = { type = "string" },
                    evidence    = { type = "string", description = "Short code snippet or pattern" },
                    fix_hint    = { type = "string" }
                },
                required = { "category", "name", "severity", "location", "description" }
            }
        },
        metrics = {
            type = "object",
            properties = {
                fn_count            = { type = "integer" },
                longest_fn_lines    = { type = "integer" },
                struct_count        = { type = "integer" },
                enum_count          = { type = "integer" },
                unwrap_count        = { type = "integer" },
                expect_count        = { type = "integer" },
                panic_count         = { type = "integer" },
                todo_count          = { type = "integer" },
                unsafe_count        = { type = "integer" },
                clone_call_count    = { type = "integer" },
                stringly_typed      = { type = "integer", description = "Count of functions taking String where &str would do" },
                missing_docs_public = { type = "integer" },
                dead_code_markers   = { type = "integer", description = "allow(dead_code), #[allow(unused)], etc." }
            }
        },
        score = { type = "integer", description = "1-10 quality score for this file" }
    },
    required = { "file", "kind", "summary", "line_count", "smells", "metrics", "score" }
}

local CROSS_SCHEMA = {
    type = "object",
    properties = {
        duplications = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    description = { type = "string" },
                    locations   = { type = "array", items = { type = "string" } },
                    lines_dup   = { type = "integer" },
                    severity    = { type = "string" }
                },
                required = { "description", "locations", "severity" }
            }
        },
        inconsistencies = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    dimension = { type = "string", description = "e.g. 'error handling', 'logging style', 'naming'" },
                    example_a = { type = "string" },
                    example_b = { type = "string" },
                    severity  = { type = "string" }
                },
                required = { "dimension", "example_a", "example_b", "severity" }
            }
        },
        scattered_concerns = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    concern = { type = "string" },
                    files   = { type = "array", items = { type = "string" } },
                    fix     = { type = "string" }
                },
                required = { "concern", "files", "fix" }
            }
        },
        missing_abstractions = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    proposed_trait_or_struct = { type = "string" },
                    rationale                = { type = "string" },
                    consumers                = { type = "array", items = { type = "string" } }
                },
                required = { "proposed_trait_or_struct", "rationale" }
            }
        }
    },
    required = { "duplications", "inconsistencies", "scattered_concerns", "missing_abstractions" }
}

local SYNTH_SCHEMA = {
    type = "object",
    properties = {
        overall_score     = { type = "integer", description = "1-10 module-level score" },
        executive_summary = { type = "string" },
        top_10_smells = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    rank     = { type = "integer" },
                    title    = { type = "string" },
                    file     = { type = "string" },
                    category = { type = "string" },
                    severity = { type = "string" },
                    impact   = { type = "string", description = "Why it matters" },
                    fix      = { type = "string" },
                    effort   = { type = "string", description = "S | M | L" }
                },
                required = { "rank", "title", "file", "category", "severity", "impact", "fix", "effort" }
            }
        },
        by_file = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    file       = { type = "string" },
                    score      = { type = "integer" },
                    top_issues = { type = "array", items = { type = "string" } }
                },
                required = { "file", "score", "top_issues" }
            }
        },
        by_category = {
            type = "object",
            description = "Counts of smells grouped by Fowler's taxonomy + Rust-specific"
        },
        rust_specific_issues = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    issue = { type = "string" },
                    files = { type = "array", items = { type = "string" } },
                    fix   = { type = "string" }
                },
                required = { "issue", "files", "fix" }
            }
        },
        refactor_recommendations = {
            type = "array",
            items = {
                type = "object",
                properties = {
                    order         = { type = "integer" },
                    title         = { type = "string" },
                    rationale     = { type = "string" },
                    steps         = { type = "array", items = { type = "string" } },
                    prerequisites = { type = "array", items = { type = "string" } }
                },
                required = { "order", "title", "rationale", "steps" }
            }
        },
        positive_observations = {
            type = "array",
            items = { type = "string" }
        }
    },
    required = {
        "overall_score", "executive_summary", "top_10_smells",
        "by_file", "by_category", "rust_specific_issues",
        "refactor_recommendations", "positive_observations"
    }
}

local SMELL_HUNT_PROMPT_TEMPLATE = [[You are a senior Rust engineer performing a code smell hunt on a single source file from the Luft project (Rust multi-agent orchestration runtime at C:\Users\heycj\dev\luft).

# Target file
Path: %s
Architectural role: %s
Responsibility: %s

# Your task
1. Read the target file in full using your Read tool.
2. Also read src/adapters/mod.rs to understand the public surface this file participates in.
3. Also grep for any uses of public types/functions defined in the target file across the workspace (use the Grep tool with the type/function name as query) to understand downstream consumers and detect potential dispensable APIs.
4. Hunt for code smells and return a structured report.

# Smell taxonomy (use these exact category strings)

Fowler's 5 categories:
- Bloaters - Long Method (>50 lines), Large Struct/Module, Long Parameter List (>5 args), Data Clumps, Primitive Obsession
- OO-Abusers - Switch-on-type with no polymorphism, Refused Bequest (impl that ignores parent contract), Temporary Field, Alternative Classes With Different Interfaces
- Change-Preventers - Divergent Change (one file changes for many reasons), Shotgun Surgery (one change touches many files), Parallel Hierarchies
- Dispensables - Comments (stating what code does instead of why), Duplicate Code, Dead Code, Lazy Class/Module, Speculative Generality, Data Class
- Couplers - Feature Envy, Inappropriate Intimacy, Message Chains, Middle Man, Incomplete Library Class

Rust-specific category - use the string Rust-Specific:
- Unwrap Abuse - .unwrap() / .expect() in non-test code paths
- Panic in Library - panic! / unreachable! / todo! reachable from public API
- Excessive Clone - .clone() calls that could be borrow / Rc / Arc
- Stringly-Typed - String parameters where &str / AsRef<str> / enum would do
- Arc<Mutex<>> Smell - Arc<Mutex<T>> where channels or single-owner would be cleaner
- Public Surface Bloat - pub items that should be pub(crate) or private
- Error Type Duplication - multiple ad-hoc error enums that should be unified
- Missing Context - error propagation without .context() / .with_context() (anyhow-style)
- Async/Sync Mix - tokio::spawn of sync work, blocking calls in async fn, mutex held across .await
- Unsafe Without Justification - unsafe block without // SAFETY: comment
- Missing #[must_use] - Result/Option-returning public fns without #[must_use]
- Magic Numbers - unexplained integer/string literals
- Dead Test Code - #[cfg(test)] that does not actually test, or tests with no assertions

# Required output (JSON, schema-validated)
Return a single JSON object with the schema fields above. Be specific:
- Reference actual function names and line numbers in location (e.g. "AcpAdapter::run, L420")
- Include a 1-2 line evidence snippet for non-trivial smells
- For metrics, count by grepping the file (do not guess)
- score 1-10 where 10 = pristine, 1 = needs full rewrite

# Anti-patterns to avoid
- Do not report stylistic preferences (e.g. "could use iterator instead of for loop")
- Do not fabricate smells - if the file is clean, return an empty smells array and score it 9-10
- Do not report missing tests as a smell here (that's a coverage concern, not a smell)
- Do not summarize the file's purpose in detail - summary is one sentence max]]

function main()
    budget(600000, 60)

    phase("smell_hunt", #ADAPTERS_FILES)
    local per_file = parallel(ADAPTERS_FILES, function(f)
        local basename = f.path:match("([^/]+)$"):gsub("%.rs$", "")
        local p = string.format(SMELL_HUNT_PROMPT_TEMPLATE, f.path, f.kind, f.role)
        return {
            name        = "smell-" .. basename,
            description = "Hunt for code smells in " .. f.path .. " (" .. f.kind .. ")",
            prompt      = p,
            schema      = FILE_SUMMARY
        }
    end)

    local ok_count = 0
    local total_smells = 0
    for _, r in ipairs(per_file) do
        if r.ok then
            ok_count = ok_count + 1
            if r.output and r.output.smells then
                total_smells = total_smells + #r.output.smells
            end
        end
    end
    log(string.format("smell_hunt: %d/%d files ok, %d raw smells",
        ok_count, #ADAPTERS_FILES, total_smells))

    phase("cross_reference", 1)
    local cross = agent({
        name        = "cross-reference",
        description = "Find cross-file smells across all 5 per-file reports (duplications, inconsistencies, scattered concerns, missing abstractions)",
        prompt = "You are the cross-file analysis lead for a code smell audit of the Luft src/adapters/ module "
            .. "(Rust multi-agent orchestration runtime at C:\\Users\\heycj\\dev\\luft). "
            .. "Five per-file smell reports have been collected (one per .rs file in the module). "
            .. "Your job is to find smells that only become visible when comparing files side-by-side.\n\n"
            .. "Per-file smell reports:\n" .. json.encode(per_file) .. "\n\n"
            .. "Module file map:\n" .. json.encode(ADAPTERS_FILES) .. "\n\n"
            .. "Tasks:\n"
            .. "1. Duplications - find logic that appears in 2+ files (error mapping, retry loops, type conversion, log formatting). "
            .. "Cite both file:line locations and estimate duplicated lines.\n"
            .. "2. Inconsistencies - find places where the same kind of thing is done differently across files "
            .. "(e.g. one file uses anyhow::Result, another uses custom AdapterError; one uses tracing::error!, another uses eprintln!; "
            .. "one uses #[must_use], another does not). Pick the 3-5 most impactful dimensions.\n"
            .. "3. Scattered concerns - find behaviors that are split across multiple files but should be unified "
            .. "(e.g. ACP session lifecycle, retry policy, timeout config, error mapping each living in different files).\n"
            .. "4. Missing abstractions - find places where a new trait, struct, or module would consolidate scattered logic. "
            .. "Propose concrete names and list the call sites that would benefit.\n\n"
            .. "Use the Read/Grep tools to verify your claims by spot-checking the actual source. "
            .. "Return a JSON object matching the cross-reference schema. Be ruthlessly specific - "
            .. "every claim must be verifiable in the source code.",
        schema = CROSS_SCHEMA
    })

    local dup_count = 0
    if cross.ok and cross.output and cross.output.duplications then
        dup_count = #cross.output.duplications
    end
    log("cross_reference: " .. dup_count .. " cross-file duplications found")

    phase("synthesize", 1)
    local synth = agent({
        name        = "synthesize",
        description = "Lead reviewer aggregating per-file + cross-file smells into a prioritized, actionable final report",
        prompt = "You are the lead reviewer synthesizing a code smell audit of the Luft src/adapters/ module "
            .. "(Rust multi-agent orchestration runtime at C:\\Users\\heycj\\dev\\luft). "
            .. "Aggregate the per-file smell reports and the cross-reference findings into a prioritized, actionable final report.\n\n"
            .. "Per-file smell reports:\n" .. json.encode(per_file) .. "\n\n"
            .. "Cross-reference findings:\n" .. json.encode(cross.output) .. "\n\n"
            .. "Module file map:\n" .. json.encode(ADAPTERS_FILES) .. "\n\n"
            .. "Produce a final report with:\n"
            .. "1. overall_score (1-10) for the module\n"
            .. "2. executive_summary (2-3 paragraphs: what the module does well, what hurts it most, biggest risk)\n"
            .. "3. top_10_smells - the 10 most impactful smells ranked, with file, category, severity, why it matters, and how to fix\n"
            .. "4. by_file - per-file scores and top 3 issues per file\n"
            .. "5. by_category - counts grouped by Fowler's taxonomy (Bloaters, OO-Abusers, Change-Preventers, Dispensables, Couplers) + Rust-Specific\n"
            .. "6. rust_specific_issues - Rust-only smells with files affected and proposed fix\n"
            .. "7. refactor_recommendations - an ordered list (5-8 items) of refactor work, each with rationale and concrete steps. "
            .. "Order by: highest impact / lowest effort first. Use 'S' < 1h, 'M' < half-day, 'L' > half-day for effort.\n"
            .. "8. positive_observations - 3-5 things the module does well (do not be a doomer; balanced reports are credible)\n\n"
            .. "Be decisive. If the module is healthy, say so with a high score. If it has systemic issues, "
            .. "say that too - do not soften findings. The user wants surgical, prioritized action items, not diplomatic fluff.",
        schema = SYNTH_SCHEMA
    })

    local total_files_scored = 0
    local sum_scores = 0
    local by_severity = { high = 0, medium = 0, low = 0 }
    for _, r in ipairs(per_file) do
        if r.ok and r.output then
            if r.output.score then
                sum_scores = sum_scores + r.output.score
                total_files_scored = total_files_scored + 1
            end
            if r.output.smells then
                for _, s in ipairs(r.output.smells) do
                    if s.severity and by_severity[s.severity] ~= nil then
                        by_severity[s.severity] = by_severity[s.severity] + 1
                    end
                end
            end
        end
    end

    report({
        workflow = "adapters_smell_hunt",
        project = "luft v0.1.0 (Rust multi-agent orchestration runtime)",
        scope = "src/adapters/ (5 files, 1 agent per file)",
        files_analyzed = ok_count,
        files_total = #ADAPTERS_FILES,
        total_smells_intra_file = total_smells,
        severity_breakdown = by_severity,
        average_file_score = total_files_scored > 0 and (sum_scores / total_files_scored) or nil,
        per_file = per_file,
        cross_reference = cross.output,
        synthesis = synth.output
    })
end
