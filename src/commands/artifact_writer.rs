//! Event consumer that writes Markdown artifact reports for each agent.
//!
//! Subscribes to the same `broadcast` event stream as [`PhaseRenderer`] and
//! [`EventLogger`]. For each agent that completes, writes a structured
//! Markdown report to `{base}/{seq:02}_{name}/report.md`. At the end of a
//! run, writes a run-level summary.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use maestro::core::contract::backend::AgentStatus;
use maestro::core::contract::event::{AgentEvent, ProgressDelta, RunStatus};
use maestro::core::contract::ids::{AgentId, PhaseId, TokenUsage};

// ── State ─────────────────────────────────────────────────

#[derive(Default)]
struct AgentStats {
    agent_seq: u32,
    name: Option<String>,
    model: Option<String>,
    phase_label: Option<String>,
    messages: u32,
    tool_calls: HashMap<String, u32>,
    file_edits: Vec<PathBuf>,
    pipeline_stage: Option<usize>,
}

#[derive(Clone)]
struct AgentDoneRecord {
    agent_seq: u32,
    name: Option<String>,
    agent_id: AgentId,
    status: AgentStatus,
    tokens: TokenUsage,
    elapsed_ms: u64,
    output: serde_json::Value,
    findings: Vec<maestro::core::contract::finding::Finding>,
    prompt: String,
    retry_count: u32,
}
struct PipelineItemRecord {
    item_index: usize,
    stage_results: Vec<Option<(AgentStatus, TokenUsage, u64)>>,
}

struct PipelineContext {
    total_stages: usize,
    total_items: usize,
    current_stage: usize,
    stage_labels: Vec<String>,
    items: Vec<PipelineItemRecord>,
    pipeline_index: usize,
}

struct ParallelContext {
    count: usize,
    parallel_index: usize,
}

// ── Writer ────────────────────────────────────────────────

pub struct ArtifactWriter {
    base: PathBuf,
    agents: HashMap<AgentId, AgentStats>,
    completed_agents: Vec<AgentDoneRecord>,
    phases: HashMap<PhaseId, String>,
    pipeline_ctx: Option<PipelineContext>,
    parallel_ctxs: Vec<ParallelContext>,
    pipeline_count: usize,
    parallel_count: usize,
    task_description: Option<String>,
    run_status: Option<RunStatus>,
    run_total_tokens: Option<TokenUsage>,
    final_report: Option<serde_json::Value>,
}

impl ArtifactWriter {
    pub fn new(base: impl Into<PathBuf>, _run_id: maestro::core::contract::ids::RunId) -> Self {
        Self {
            base: base.into(),
            agents: HashMap::new(),
            completed_agents: Vec::new(),
            phases: HashMap::new(),
            pipeline_ctx: None,
            parallel_ctxs: Vec::new(),
            pipeline_count: 0,
            parallel_count: 0,
            task_description: None,
            run_status: None,
            run_total_tokens: None,
            final_report: None,
        }
    }

    pub fn handle(&mut self, evt: &AgentEvent) {
        match evt {
            AgentEvent::RunStarted { task, .. } => {
                self.task_description = Some(task.clone());
            }

            AgentEvent::PhaseStarted {
                phase_id, label, ..
            } => {
                self.phases.insert(*phase_id, label.clone());
            }

            AgentEvent::AgentStarted {
                agent_id,
                model,
                phase_id,
                name,
                agent_seq,
                ..
            } => {
                let stats = self.agents.entry(*agent_id).or_default();
                stats.model = model.clone();
                stats.phase_label = self.phases.get(phase_id).cloned();
                stats.name = name.clone();
                stats.agent_seq = *agent_seq;
                if let Some(ctx) = &self.pipeline_ctx {
                    stats.pipeline_stage = Some(ctx.current_stage);
                }
            }

            AgentEvent::AgentProgress {
                run_id: _,
                agent_id,
                delta,
            } => {
                if let Some(stats) = self.agents.get_mut(agent_id) {
                    match delta {
                        ProgressDelta::Message { .. } => stats.messages += 1,
                        ProgressDelta::ToolCall { name, .. } => {
                            *stats.tool_calls.entry(name.clone()).or_default() += 1;
                        }
                        ProgressDelta::FileEdit { path } => {
                            stats.file_edits.push(path.clone());
                        }
                        ProgressDelta::Tokens { .. } => {}
                    }
                }
            }

            AgentEvent::AgentDone {
                run_id: _,
                agent_id,
                status,
                tokens,
                elapsed_ms,
                name,
                agent_seq,
                output,
                findings,
                prompt,
                retry_count,
            } => {
                let stats = self.agents.remove(agent_id).unwrap_or_default();
                let record = AgentDoneRecord {
                    agent_seq: *agent_seq,
                    name: name.clone().or_else(|| stats.name.clone()),
                    agent_id: *agent_id,
                    status: status.clone(),
                    tokens: *tokens,
                    elapsed_ms: *elapsed_ms,
                    output: output.clone(),
                    findings: findings.clone(),
                    prompt: prompt.clone(),
                    retry_count: *retry_count,
                };

                // Track pipeline item completion
                if let Some(ctx) = &mut self.pipeline_ctx {
                    // We don't have item_index from AgentDone; pipeline
                    // tracking is handled by PipelineItemDone events.
                    let _ = ctx;
                }

                let _ = self.write_agent_report(&record, &stats);
                self.completed_agents.push(record);
            }

            AgentEvent::PipelineStarted {
                total_stages,
                items,
                ..
            } => {
                let idx = self.pipeline_count;
                self.pipeline_count += 1;
                self.pipeline_ctx = Some(PipelineContext {
                    total_stages: *total_stages,
                    total_items: *items,
                    current_stage: 0,
                    stage_labels: vec![String::new(); *total_stages],
                    items: (0..*items)
                        .map(|i| PipelineItemRecord {
                            item_index: i,
                            stage_results: vec![None; *total_stages],
                        })
                        .collect(),
                    pipeline_index: idx,
                });
            }

            AgentEvent::PipelineStageStarted {
                stage_index, label, ..
            } => {
                if let Some(ctx) = &mut self.pipeline_ctx {
                    ctx.current_stage = *stage_index;
                    if let Some(slot) = ctx.stage_labels.get_mut(*stage_index) {
                        *slot = label.clone();
                    }
                }
            }

            AgentEvent::PipelineItemDone {
                stage_index,
                item_index,
                status,
                tokens,
                elapsed_ms,
                ..
            } => {
                if let Some(ctx) = &mut self.pipeline_ctx {
                    if let Some(item) = ctx.items.iter_mut().find(|i| i.item_index == *item_index) {
                        if let Some(slot) = item.stage_results.get_mut(*stage_index) {
                            *slot = Some((status.clone(), *tokens, *elapsed_ms));
                        }
                    }
                }
            }

            AgentEvent::PipelineDone { .. } => {
                if let Some(ctx) = self.pipeline_ctx.take() {
                    let _ = self.write_pipeline_summary(&ctx);
                }
            }

            AgentEvent::ParallelStarted { count, .. } => {
                let idx = self.parallel_count;
                self.parallel_count += 1;
                self.parallel_ctxs.push(ParallelContext {
                    count: *count,
                    parallel_index: idx,
                });
            }

            AgentEvent::ParallelDone {
                ok,
                failed,
                elapsed_ms,
                ..
            } => {
                let pctx = self.parallel_ctxs.pop();
                let _ = self.write_parallel_summary(*ok, *failed, *elapsed_ms, pctx.as_ref());
            }

            AgentEvent::ReportEmitted { report, .. } => {
                self.final_report = Some(report.clone());
                let _ = self.write_report_file();
            }

            AgentEvent::RunDone {
                status,
                total_tokens,
                report,
                ..
            } => {
                self.run_status = Some(*status);
                self.run_total_tokens = Some(*total_tokens);
                if self.final_report.is_none() {
                    self.final_report = Some(report.clone());
                }
                let _ = self.write_run_summary();
            }

            _ => {}
        }
    }

    // ── Directory helpers ──────────────────────────────────

    fn agent_dir_name(seq: u32, name: &Option<String>) -> String {
        match name {
            Some(n) => format!("{:02}_{}", seq, n),
            None => format!("{:02}", seq),
        }
    }

    fn agent_dir(&self, seq: u32, name: &Option<String>) -> PathBuf {
        self.base.join(Self::agent_dir_name(seq, name))
    }

    // ── Agent report ───────────────────────────────────────

    fn write_agent_report(
        &self,
        record: &AgentDoneRecord,
        stats: &AgentStats,
    ) -> std::io::Result<()> {
        let dir = self.agent_dir(record.agent_seq, &record.name);
        fs::create_dir_all(&dir)?;
        let path = dir.join("report.md");
        let mut f = fs::File::create(&path)?;
        let md = self.render_agent_markdown(record, stats);
        f.write_all(md.as_bytes())?;
        tracing::debug!(path = %path.display(), "wrote agent artifact");
        Ok(())
    }

    fn render_agent_markdown(&self, record: &AgentDoneRecord, stats: &AgentStats) -> String {
        let mut s = String::with_capacity(2048);

        // Title
        match &record.name {
            Some(n) => writeln!(s, "# Agent #{:0>2} `{}`\n", record.agent_seq, n).unwrap(),
            None => writeln!(s, "# Agent #{:0>2}\n", record.agent_seq).unwrap(),
        }

        // Description line
        if let Some(label) = &stats.phase_label {
            writeln!(s, "> {}\n", label).unwrap();
        }

        // Metadata
        writeln!(s, "## Metadata\n").unwrap();
        writeln!(s, "| Field    | Value                     |").unwrap();
        writeln!(s, "|----------|---------------------------|").unwrap();
        writeln!(
            s,
            "| Seq      | {}                         |",
            record.agent_seq
        )
        .unwrap();
        match &record.name {
            Some(n) => writeln!(s, "| Name     | {}                         |", n).unwrap(),
            None => writeln!(s, "| Name     | -                         |").unwrap(),
        }
        writeln!(s, "| Agent ID | {:.12}... |", record.agent_id).unwrap();
        writeln!(
            s,
            "| Status   | {:?}                       |",
            record.status
        )
        .unwrap();
        match &stats.model {
            Some(m) => writeln!(s, "| Model    | {} |", m).unwrap(),
            None => writeln!(s, "| Model    | -                         |").unwrap(),
        }
        match &stats.phase_label {
            Some(l) => writeln!(s, "| Phase    | {} |", l).unwrap(),
            None => writeln!(s, "| Phase    | -                         |").unwrap(),
        }
        if let Some(stage) = stats.pipeline_stage {
            writeln!(s, "| Pipeline | Stage {}                  |", stage).unwrap();
        }
        writeln!(
            s,
            "| Elapsed  | {:.1}s                     |",
            record.elapsed_ms as f64 / 1000.0
        )
        .unwrap();
        writeln!(
            s,
            "| Retries  | {}                         |",
            record.retry_count
        )
        .unwrap();

        // Token Usage
        writeln!(s, "## Token Usage\n").unwrap();
        writeln!(s, "| Metric      | Count   |").unwrap();
        writeln!(s, "|-------------|---------|").unwrap();
        writeln!(s, "| Input       | {:>6}  |", record.tokens.input).unwrap();
        writeln!(s, "| Output      | {:>6}  |", record.tokens.output).unwrap();
        writeln!(s, "| Cache Read  | {:>6}  |", record.tokens.cache_read).unwrap();
        writeln!(s, "| Cache Write | {:>6}  |", record.tokens.cache_write).unwrap();
        writeln!(
            s,
            "| **Total**   | **{:>6}** |\n",
            record.tokens.input + record.tokens.output
        )
        .unwrap();

        // Execution
        writeln!(s, "## Execution\n").unwrap();
        writeln!(s, "- Rounds: {}", stats.messages).unwrap();
        let total_tools: u32 = stats.tool_calls.values().sum();
        writeln!(s, "- Tool Calls: {}", total_tools).unwrap();
        let mut sorted_tools: Vec<_> = stats.tool_calls.iter().collect();
        sorted_tools.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in &sorted_tools {
            writeln!(s, "  - `{}`: {}", name, count).unwrap();
        }
        if !stats.file_edits.is_empty() {
            writeln!(s, "- File Edits: {}", stats.file_edits.len()).unwrap();
            let deduped: Vec<_> = {
                let mut seen = std::collections::HashSet::new();
                stats
                    .file_edits
                    .iter()
                    .filter(|p| seen.insert(p.to_string_lossy().to_string()))
                    .collect()
            };
            for path in &deduped {
                writeln!(s, "  - `{}`", path.display()).unwrap();
            }
        }
        writeln!(s).unwrap();

        // Prompt
        if !record.prompt.is_empty() {
            writeln!(s, "## Prompt\n").unwrap();
            writeln!(s, "```\n{}\n```\n", record.prompt).unwrap();
        }

        // Output
        writeln!(s, "## Output\n").unwrap();
        match record.status {
            AgentStatus::Ok => {
                let pretty = if record.output.is_null() {
                    "(no output)".to_string()
                } else {
                    serde_json::to_string_pretty(&record.output)
                        .unwrap_or_else(|_| record.output.to_string())
                };
                writeln!(s, "```json\n{}\n```\n", pretty).unwrap();
            }
            _ => {
                writeln!(
                    s,
                    "```json\n{{ \"status\": \"{:?}\" }}\n```\n",
                    record.status
                )
                .unwrap();
            }
        }

        // Findings
        if !record.findings.is_empty() {
            writeln!(s, "## Findings\n").unwrap();
            let mut sorted = record.findings.clone();
            sorted.sort_by_key(|f| f.severity);
            for f in &sorted {
                writeln!(s, "- **{:?}**: {}", f.severity, f.title).unwrap();
            }
            writeln!(s).unwrap();
        }

        s
    }

    // ── Pipeline summary ───────────────────────────────────

    fn write_pipeline_summary(&self, ctx: &PipelineContext) -> std::io::Result<()> {
        let dir = self.base.join(format!("pipeline_{}", ctx.pipeline_index));
        fs::create_dir_all(&dir)?;
        let path = dir.join("_summary.md");
        let mut f = fs::File::create(&path)?;
        let md = self.render_pipeline_markdown(ctx);
        f.write_all(md.as_bytes())?;
        Ok(())
    }

    fn render_pipeline_markdown(&self, ctx: &PipelineContext) -> String {
        let mut s = String::with_capacity(1024);

        writeln!(
            s,
            "# Pipeline: {} stages x {} items\n",
            ctx.total_stages, ctx.total_items
        )
        .unwrap();

        // Results matrix
        let mut header = String::from("| Item |");
        let mut sep = String::from("|------|");
        for stage in 0..ctx.total_stages {
            let label = ctx
                .stage_labels
                .get(stage)
                .filter(|l| !l.is_empty())
                .map(|s| s.as_str())
                .unwrap_or("stage");
            write!(header, " Stage {} ({}) |", stage, label).unwrap();
            sep.push_str("------|");
        }
        writeln!(s, "{}", header).unwrap();
        writeln!(s, "{}", sep).unwrap();

        let mut total_ok = 0usize;
        let mut total_failed = 0usize;

        for item in &ctx.items {
            write!(s, "| {}    |", item.item_index).unwrap();
            for slot in &item.stage_results {
                match slot {
                    Some((status, tokens, elapsed_ms)) => {
                        let ok = matches!(status, AgentStatus::Ok);
                        if ok {
                            total_ok += 1;
                        } else {
                            total_failed += 1;
                        }
                        write!(
                            s,
                            " {:?} . {} tok . {:.1}s |",
                            status,
                            tokens.input + tokens.output,
                            *elapsed_ms as f64 / 1000.0
                        )
                        .unwrap();
                    }
                    None => {
                        write!(s, " -               |").unwrap();
                    }
                }
            }
            writeln!(s).unwrap();
        }

        writeln!(
            s,
            "\n**Totals**: OK {} / {} . Failed {} / {}\n",
            total_ok,
            total_ok + total_failed,
            total_failed,
            total_ok + total_failed
        )
        .unwrap();

        s
    }

    // ── Parallel summary ───────────────────────────────────

    fn write_parallel_summary(
        &self,
        ok: usize,
        failed: usize,
        elapsed_ms: u64,
        pctx: Option<&ParallelContext>,
    ) -> std::io::Result<()> {
        let Some(ctx) = pctx else {
            return Ok(());
        };
        let dir = self.base.join(format!("parallel_{}", ctx.parallel_index));
        fs::create_dir_all(&dir)?;
        let path = dir.join("_summary.md");
        let mut f = fs::File::create(&path)?;

        let mut s = String::with_capacity(512);
        writeln!(s, "# Parallel: {} items\n", ctx.count).unwrap();
        writeln!(s, "> elapsed {:.1}s\n", elapsed_ms as f64 / 1000.0).unwrap();
        writeln!(
            s,
            "**Totals**: OK {} / {} . Failed {} / {} . Elapsed {:.1}s\n",
            ok,
            ok + failed,
            failed,
            ok + failed,
            elapsed_ms as f64 / 1000.0
        )
        .unwrap();

        f.write_all(s.as_bytes())?;
        Ok(())
    }

    // ── Run report file ────────────────────────────────────

    fn write_report_file(&self) -> std::io::Result<()> {
        let Some(report) = &self.final_report else {
            return Ok(());
        };
        let path = self.base.join("_report.md");
        let mut f = fs::File::create(&path)?;
        let pretty = serde_json::to_string_pretty(report).unwrap_or_else(|_| report.to_string());
        write!(f, "# Final Report\n\n```json\n{}\n```\n", pretty)?;
        Ok(())
    }

    // ── Run summary ────────────────────────────────────────

    fn write_run_summary(&self) -> std::io::Result<()> {
        let path = self.base.join("_summary.md");
        let mut f = fs::File::create(&path)?;
        let md = self.render_run_summary();
        f.write_all(md.as_bytes())?;
        Ok(())
    }

    fn render_run_summary(&self) -> String {
        let mut s = String::with_capacity(2048);

        writeln!(s, "# Run Summary\n").unwrap();

        if let Some(task) = &self.task_description {
            let truncated: String = task.chars().take(100).collect();
            writeln!(s, "> {}\n", truncated).unwrap();
        }

        // Overview
        writeln!(s, "## Overview\n").unwrap();
        writeln!(s, "| Field         | Value    |").unwrap();
        writeln!(s, "|---------------|----------|").unwrap();
        match self.run_status {
            Some(rs) => writeln!(s, "| Status        | {:?}     |", rs).unwrap(),
            None => writeln!(s, "| Status        | -        |").unwrap(),
        }
        if let Some(t) = &self.run_total_tokens {
            writeln!(
                s,
                "| Total Tokens  | {} (in: {} / out: {}) |",
                t.input + t.output,
                t.input,
                t.output
            )
            .unwrap();
        }
        let ok_count = self
            .completed_agents
            .iter()
            .filter(|a| matches!(a.status, AgentStatus::Ok))
            .count();
        let err_count = self
            .completed_agents
            .iter()
            .filter(|a| !matches!(a.status, AgentStatus::Ok))
            .count();
        writeln!(
            s,
            "| Agents        | {} (ok: {} / error: {}) |",
            self.completed_agents.len(),
            ok_count,
            err_count
        )
        .unwrap();
        if self.pipeline_count > 0 {
            writeln!(s, "| Pipelines     | {}       |", self.pipeline_count).unwrap();
        }
        if self.parallel_count > 0 {
            writeln!(s, "| Parallels     | {}       |", self.parallel_count).unwrap();
        }
        writeln!(s).unwrap();

        // Agents table
        if !self.completed_agents.is_empty() {
            writeln!(s, "## Agents\n").unwrap();
            writeln!(
                s,
                "| # | Name | Agent ID | Status | Tokens | Rounds | Tools | Report |"
            )
            .unwrap();
            writeln!(
                s,
                "|---|------|----------|--------|--------|--------|-------|--------|"
            )
            .unwrap();

            let mut sorted = self.completed_agents.clone();
            sorted.sort_by_key(|a| a.agent_seq);

            for a in &sorted {
                let name = a.name.clone().unwrap_or_else(|| "-".into());
                let id_short: String = format!("{:.12}", a.agent_id);
                let tokens = a.tokens.input + a.tokens.output;
                // Rounds and tools not tracked in completed_agents; show -
                writeln!(
                    s,
                    "| {:0>2} | {} | {}... | {:?} | {} | - | - | [report](./{}/report.md) |",
                    a.agent_seq,
                    name,
                    id_short,
                    a.status,
                    tokens,
                    Self::agent_dir_name(a.agent_seq, &a.name),
                )
                .unwrap();
            }
            writeln!(s).unwrap();
        }

        // Errors
        let errors: Vec<_> = self
            .completed_agents
            .iter()
            .filter(|a| !matches!(a.status, AgentStatus::Ok))
            .collect();
        if !errors.is_empty() {
            writeln!(s, "## Errors\n").unwrap();
            writeln!(s, "| # | Name | Status |").unwrap();
            writeln!(s, "|---|------|--------|").unwrap();
            for a in &errors {
                let name = a.name.clone().unwrap_or_else(|| "-".into());
                writeln!(s, "| {:0>2} | {} | {:?} |", a.agent_seq, name, a.status).unwrap();
            }
            writeln!(s).unwrap();
        }

        // Final report
        if let Some(report) = &self.final_report {
            writeln!(s, "## Final Report\n").unwrap();
            let pretty =
                serde_json::to_string_pretty(report).unwrap_or_else(|_| report.to_string());
            writeln!(s, "```json\n{}\n```\n", pretty).unwrap();
        }

        s
    }
}
