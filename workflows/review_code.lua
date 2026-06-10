meta = {
    phases = {
        {
            label = "discovery",
            detail = "Enumerate all Rust source files and group by module",
            agents = 1,
            depends_on = {}
        },
        {
            label = "review",
            detail = "Deep parallel review per module group",
            agents = 8,
            depends_on = { 0 }
        },
        {
            label = "verify",
            detail = "Adversarial cross-verification of critical findings",
            agents = 4,
            depends_on = { 1 }
        },
        {
            label = "synthesize",
            detail = "Aggregate all module reviews and verified findings into final report",
            agents = 1,
            depends_on = { 2 }
        }
    },
    reasoning = "Four-stage pipeline: first discover and group all source files, then fan-out to review each module group in parallel, then run adversarial converge to cross-check and validate critical findings, and finally synthesize everything into a structured final report. The verify stage reduces false positives by having findings survive adversarial scrutiny before reaching the final output."
}

function main()
    budget(300000, 30)

    phase("discovery", 1)
    local discovery = agent({
        prompt = "You are reviewing the Maestro project — a Rust multi-agent orchestration runtime at C:\\Users\\heycj\\dev\\maestro. "
            .. "First, explore the repository using your Read tool to discover ALL Rust source files (*.rs), excluding target/ and any generated directories. "
            .. "For each file, provide: its relative path, line count, and a 1-sentence summary of its purpose. "
            .. "Then group the files by module category using these rules:\n"
            .. "  - 'core'       = files under src/core/\n"
            .. "  - 'runtime'    = files under src/runtime/\n"
            .. "  - 'adapters'   = files under src/adapters/\n"
            .. "  - 'planner'    = files under src/planner/\n"
            .. "  - 'tui'        = files under src/tui/\n"
            .. "  - 'cli'        = src/cli.rs, src/main.rs\n"
            .. "  - 'lib'        = src/lib.rs\n"
            .. "  - 'mcp'        = src/mcp.rs\n"
            .. "  - 'tests'      = files under tests/\n"
            .. "Return a JSON object with key 'groups' mapping group names to arrays of {path, lines, purpose} objects. "
            .. "Be thorough — aim to cover every .rs file in the project."
    })
    local groups = discovery.output.groups or {}
    local group_names = {}
    for k, _ in pairs(groups) do
        table.insert(group_names, k)
    end
    log("Discovered " .. #group_names .. " module groups for review")

    phase("review", #group_names)
    local group_reviews = parallel(group_names, function(gname)
        local files = groups[gname]
        local file_list = json.encode(files)
        return {
            prompt = "You are a senior Rust engineer conducting a thorough code review of the Maestro project. "
                .. "Review the files in module group '" .. gname .. "'. "
                .. "Read EACH file in full using your Read tool, then produce a detailed review. "
                .. "Files to review: " .. file_list .. "\n\n"
                .. "For each file assess:\n"
                .. "1. Code quality & idiomatic Rust (error handling, unwraps, expects, unsafe blocks, async patterns)\n"
                .. "2. Architecture & design (separation of concerns, coupling, trait vs concrete types)\n"
                .. "3. Safety & correctness (potential bugs, race conditions, edge cases, panic paths)\n"
                .. "4. Documentation (missing docs, unclear logic, stale comments)\n"
                .. "5. Performance (unnecessary allocations, clones, Arc usage, lock contention)\n\n"
                .. "Return a JSON object with:\n"
                .. "  - 'module': the group name\n"
                .. "  - 'files': array of {path, issues_found=<int>, summary=<string>, quality_score=<1-10>}\n"
                .. "  - 'critical': array of {file, line_hint, description} for HIGH-severity issues (actual bugs, unsoundness, panics)\n"
                .. "  - 'warnings': array of {file, description} for moderate issues\n"
                .. "  - 'suggestions': array of {file, description} for style/performance improvements\n"
                .. "  - 'positive': array of {file, description} for things done well\n"
                .. "  - 'module_score': integer 1-10 overall quality for this module\n"
                .. "Be specific — reference actual code patterns, line numbers, and function names."
        }
    end)

    phase("verify", 1)
    local all_critical = {}
    for _, r in ipairs(group_reviews) do
        if r.ok and r.output and r.output.critical then
            for _, c in ipairs(r.output.critical) do
                table.insert(all_critical, c)
            end
        end
    end
    log("Found " .. #all_critical .. " critical findings to verify")

    local verified = agent({
        prompt = "You are the verification lead for a code review of the Maestro Rust project. "
            .. "Below are " .. #all_critical .. " critical findings reported by module reviewers. "
            .. "Your job is to cross-verify each one: read the relevant source code, confirm or refute the finding, "
            .. "and return a consolidated, verified list. Eliminate false positives and duplicates.\n\n"
            .. "Raw critical findings:\n" .. json.encode(all_critical) .. "\n\n"
            .. "Return a JSON object with:\n"
            .. "  - 'verified_findings': array of {file, description, severity='critical', confirmed=<bool>, notes=<string>}\n"
            .. "  - 'false_positives': array of {original_claim, reason_for_rejection}\n"
            .. "  - 'duplicates_merged': number\n"
            .. "  - 'net_critical_count': number of confirmed unique critical issues"
    })

    phase("synthesize", 1)
    local synthesis = agent({
        prompt = "You are the lead reviewer synthesizing results from a full code review of the Maestro Rust project. "
            .. "Below are module-level reviews and the verified critical findings. "
            .. "Aggregate everything into a final comprehensive report.\n\n"
            .. "Module reviews:\n" .. json.encode(group_reviews) .. "\n\n"
            .. "Verified findings:\n" .. json.encode(verified) .. "\n\n"
            .. "Return a JSON object with:\n"
            .. "  - 'overall_score': integer 1-10\n"
            .. "  - 'executive_summary': 2-3 paragraph summary of codebase health\n"
            .. "  - 'critical_findings': array of {module, file, description, notes}\n"
            .. "  - 'top_recommendations': array of the 5 most impactful changes to make (ordered by priority)\n"
            .. "  - 'strengths': what the project does well architecturally\n"
            .. "  - 'module_scores': {module_name: <score>}\n"
            .. "  - 'total_files_reviewed': number\n"
            .. "  - 'verified_false_positives': number of eliminated claims\n"
            .. "  - 'coverage_notes': any modules or patterns that need deeper investigation"
    })

    local reviewed_count = 0
    local total_score = 0
    local scored_modules = 0
    for _, r in ipairs(group_reviews) do
        if r.ok and r.output then
            if r.output.files then
                reviewed_count = reviewed_count + #r.output.files
            end
            if r.output.module_score then
                total_score = total_score + r.output.module_score
                scored_modules = scored_modules + 1
            end
        end
    end

    local verified_count = 0
    if verified.ok and verified.output and verified.output.verified_findings then
        for _, v in ipairs(verified.output.verified_findings) do
            if v.confirmed then
                verified_count = verified_count + 1
            end
        end
    end

    report({
        workflow = "code_review",
        project = "maestro v0.1.0 (Rust multi-agent orchestration runtime)",
        modules_reviewed = #group_names,
        files_analyzed = reviewed_count,
        average_module_score = scored_modules > 0 and (total_score / scored_modules) or nil,
        critical_verified_count = verified_count,
        false_positives_eliminated = verified.ok and verified.output and verified.output.false_positives and #verified.output.false_positives or 0,
        overall = synthesis.output,
        module_reviews = group_reviews,
        verification = verified.output
    })
end
