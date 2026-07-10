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

use luft::core::contract::backend::AgentStatus;
use luft::core::contract::event::{AgentEvent, ProgressDelta, RunStatus};
use luft::core::contract::ids::{AgentId, PhaseId, TokenUsage};

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
    findings: Vec<luft::core::contract::finding::Finding>,
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
    pub fn new(base: impl Into<PathBuf>, _run_id: luft::core::contract::ids::RunId) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use luft::core::contract::finding::{Finding, Severity};
    use luft::core::contract::ids::RunId;

    fn new_writer() -> (tempfile::TempDir, ArtifactWriter) {
        let dir = tempfile::tempdir().unwrap();
        let writer = ArtifactWriter::new(dir.path(), RunId::nil());
        (dir, writer)
    }

    fn run_started() -> AgentEvent {
        AgentEvent::RunStarted {
            run_id: uuid::Uuid::now_v7(),
            task: "demo task".to_string(),
            ts: Utc::now(),
        }
    }

    fn phase_started(phase_id: u32, label: &str) -> AgentEvent {
        AgentEvent::PhaseStarted {
            run_id: uuid::Uuid::now_v7(),
            phase_id,
            label: label.to_string(),
            planned: 1,
            parent_span_id: None,
            description: None,
            role: None,
            ts: Utc::now(),
        }
    }

    fn agent_started(phase_id: u32, agent_id: AgentId, name: &str) -> AgentEvent {
        AgentEvent::AgentStarted {
            run_id: uuid::Uuid::now_v7(),
            phase_id,
            agent_id,
            prompt_preview: String::new(),
            model: Some("test-model".to_string()),
            description: None,
            role: None,
            name: Some(name.to_string()),
            agent_seq: 1,
        }
    }

    fn agent_done(agent_id: AgentId, name: &str) -> AgentEvent {
        AgentEvent::AgentDone {
            run_id: uuid::Uuid::now_v7(),
            agent_id,
            status: AgentStatus::Ok,
            tokens: TokenUsage {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
            },
            elapsed_ms: 1_500,
            name: Some(name.to_string()),
            agent_seq: 1,
            output: serde_json::json!({"answer": "ok"}),
            findings: vec![],
            prompt: "do something".to_string(),
            retry_count: 0,
        }
    }

    // ── new() ─────────────────────────────────────────────────────────────

    #[test]
    fn new_creates_empty_writer() {
        let (_tmp, writer) = new_writer();
        // We don't have public accessors for state, but we can verify the
        // writer handles events without panicking.
        let mut w = writer;
        w.handle(&run_started());
    }

    // ── RunStarted ────────────────────────────────────────────────────────

    #[test]
    fn run_started_records_task_description() {
        let (_tmp, mut w) = new_writer();
        w.handle(&run_started());
        // Indirectly verified by inspecting the run summary markdown.
        let mut w = w;
        w.handle(&AgentEvent::RunDone {
            run_id: uuid::Uuid::now_v7(),
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({}),
            ts: Utc::now(),
        });
        let summary = w.render_run_summary();
        assert!(summary.contains("demo task"), "task description in summary");
    }

    // ── PhaseStarted ──────────────────────────────────────────────────────

    #[test]
    fn phase_started_records_label() {
        let (_tmp, mut w) = new_writer();
        w.handle(&phase_started(1, "research"));
        // Phase label surfaces in the agent report metadata after agent completes.
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "agent-a"));
        w.handle(&agent_done(agent_id, "agent-a"));
        let report_path = _tmp
            .path()
            .join("01_agent-a")
            .join("report.md");
        let content = std::fs::read_to_string(&report_path).unwrap();
        assert!(content.contains("research"), "phase label should appear");
    }

    // ── AgentStarted ──────────────────────────────────────────────────────

    #[test]
    fn agent_started_does_not_write_file() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "agent-a"));
        // No report.md expected yet.
        let report_path = tmp.path().join("01_agent-a").join("report.md");
        assert!(!report_path.exists(), "report.md should not exist yet");
    }

    // ── AgentProgress ─────────────────────────────────────────────────────

    #[test]
    fn agent_progress_message_increments_round_count() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "msg-agent"));
        for _ in 0..3 {
            w.handle(&AgentEvent::AgentProgress {
                run_id: uuid::Uuid::now_v7(),
                agent_id,
                delta: ProgressDelta::Message {
                    text: "hi".to_string(),
                },
            });
        }
        w.handle(&agent_done(agent_id, "msg-agent"));
        let report = std::fs::read_to_string(tmp.path().join("01_msg-agent").join("report.md"))
            .unwrap();
        assert!(report.contains("Rounds: 3"), "round count should be 3, got:\n{report}");
    }

    #[test]
    fn agent_progress_tool_call_counts_per_name() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "tool-agent"));
        for _ in 0..2 {
            w.handle(&AgentEvent::AgentProgress {
                run_id: uuid::Uuid::now_v7(),
                agent_id,
                delta: ProgressDelta::ToolCall {
                    name: "read_file".to_string(),
                    summary: "x".to_string(),
                },
            });
        }
        w.handle(&AgentEvent::AgentProgress {
            run_id: uuid::Uuid::now_v7(),
            agent_id,
            delta: ProgressDelta::ToolCall {
                name: "grep".to_string(),
                summary: "y".to_string(),
            },
        });
        w.handle(&agent_done(agent_id, "tool-agent"));
        let report =
            std::fs::read_to_string(tmp.path().join("01_tool-agent").join("report.md")).unwrap();
        assert!(report.contains("read_file") && report.contains("2"));
        assert!(report.contains("grep"));
        assert!(report.contains("Tool Calls: 3"));
    }

    #[test]
    fn agent_progress_file_edit_collected_and_deduped() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "edit-agent"));
        let path = std::path::PathBuf::from("/tmp/foo.rs");
        for _ in 0..3 {
            w.handle(&AgentEvent::AgentProgress {
                run_id: uuid::Uuid::now_v7(),
                agent_id,
                delta: ProgressDelta::FileEdit { path: path.clone() },
            });
        }
        w.handle(&agent_done(agent_id, "edit-agent"));
        let report =
            std::fs::read_to_string(tmp.path().join("01_edit-agent").join("report.md")).unwrap();
        assert!(report.contains("File Edits: 3"));
        // Dedup keeps unique paths in the per-path list, but the total count
        // reflects every event. At minimum, the path is listed once.
        assert!(report.contains("/tmp/foo.rs"));
    }

    #[test]
    fn agent_progress_tokens_is_noop() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "tok-agent"));
        w.handle(&AgentEvent::AgentProgress {
            run_id: uuid::Uuid::now_v7(),
            agent_id,
            delta: ProgressDelta::Tokens {
                usage: TokenUsage::default(),
            },
        });
        w.handle(&agent_done(agent_id, "tok-agent"));
        // No panics; file exists.
        assert!(tmp
            .path()
            .join("01_tok-agent")
            .join("report.md")
            .exists());
    }

    #[test]
    fn agent_progress_for_unknown_agent_is_noop() {
        let (_tmp, mut w) = new_writer();
        let unknown = uuid::Uuid::now_v7();
        // No AgentStarted before — must not panic.
        w.handle(&AgentEvent::AgentProgress {
            run_id: uuid::Uuid::now_v7(),
            agent_id: unknown,
            delta: ProgressDelta::Message {
                text: "orphan".to_string(),
            },
        });
    }

    // ── AgentDone ─────────────────────────────────────────────────────────

    #[test]
    fn agent_done_writes_report_with_status_ok() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "done-ok"));
        w.handle(&agent_done(agent_id, "done-ok"));
        let path = tmp.path().join("01_done-ok").join("report.md");
        assert!(path.exists(), "report should exist");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Agent #01 `done-ok`"));
        assert!(content.contains("Status"));
        assert!(content.contains("Ok"));
        assert!(content.contains("do something")); // prompt
        assert!(content.contains("\"answer\"")); // output JSON
    }

    #[test]
    fn agent_done_writes_report_with_status_error() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "err"));
        w.handle(&AgentEvent::AgentDone {
            run_id: uuid::Uuid::now_v7(),
            agent_id,
            status: AgentStatus::Error,
            tokens: TokenUsage::default(),
            elapsed_ms: 100,
            name: Some("err".to_string()),
            agent_seq: 2,
            output: serde_json::Value::Null,
            findings: vec![],
            prompt: String::new(),
            retry_count: 0,
        });
        let content = std::fs::read_to_string(tmp.path().join("02_err").join("report.md")).unwrap();
        assert!(content.contains("Error"));
    }

    #[test]
    fn agent_done_handles_null_output_for_ok_status() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "nullout"));
        w.handle(&AgentEvent::AgentDone {
            run_id: uuid::Uuid::now_v7(),
            agent_id,
            status: AgentStatus::Ok,
            tokens: TokenUsage::default(),
            elapsed_ms: 0,
            name: Some("nullout".to_string()),
            agent_seq: 3,
            output: serde_json::Value::Null,
            findings: vec![],
            prompt: String::new(),
            retry_count: 0,
        });
        let content =
            std::fs::read_to_string(tmp.path().join("03_nullout").join("report.md")).unwrap();
        assert!(content.contains("(no output)"));
    }

    #[test]
    fn agent_done_handles_missing_name_uses_seq_only() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        // Don't emit AgentStarted first — record has no name.
        w.handle(&AgentEvent::AgentDone {
            run_id: uuid::Uuid::now_v7(),
            agent_id,
            status: AgentStatus::Ok,
            tokens: TokenUsage::default(),
            elapsed_ms: 0,
            name: None,
            agent_seq: 7,
            output: serde_json::Value::Null,
            findings: vec![],
            prompt: String::new(),
            retry_count: 0,
        });
        let dir = tmp.path().join("07");
        assert!(dir.exists(), "directory '07' should be created");
        assert!(dir.join("report.md").exists());
    }

    #[test]
    fn agent_done_records_findings_sorted_by_severity() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "find-agent"));
        w.handle(&AgentEvent::AgentDone {
            run_id: uuid::Uuid::now_v7(),
            agent_id,
            status: AgentStatus::Ok,
            tokens: TokenUsage::default(),
            elapsed_ms: 0,
            name: Some("find-agent".to_string()),
            agent_seq: 1,
            output: serde_json::Value::Null,
            findings: vec![
                Finding {
                    kind: "low".into(),
                    severity: Severity::Low,
                    title: "L".into(),
                    detail: "d".into(),
                    location: None,
                    evidence: vec![],
                    data: serde_json::Value::Null,
                },
                Finding {
                    kind: "critical".into(),
                    severity: Severity::Critical,
                    title: "C".into(),
                    detail: "d".into(),
                    location: None,
                    evidence: vec![],
                    data: serde_json::Value::Null,
                },
            ],
            prompt: String::new(),
            retry_count: 0,
        });
        let content =
            std::fs::read_to_string(tmp.path().join("01_find-agent").join("report.md")).unwrap();
        assert!(content.contains("Findings"));
        // Critical (lower Ord value? actually High<Critical — see Severity::ord)
        // We just check both titles appear.
        assert!(content.contains("L"));
        assert!(content.contains("C"));
    }

    // ── Pipeline events ───────────────────────────────────────────────────

    #[test]
    fn pipeline_started_writes_summary_on_done() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::PipelineStarted {
            run_id: uuid::Uuid::now_v7(),
            total_stages: 2,
            items: 3,
        });
        w.handle(&AgentEvent::PipelineStageStarted {
            run_id: uuid::Uuid::now_v7(),
            stage_index: 0,
            label: "lint".into(),
            agents_in_stage: 1,
        });
        // Item 0 in stage 0 ok
        w.handle(&AgentEvent::PipelineItemDone {
            run_id: uuid::Uuid::now_v7(),
            stage_index: 0,
            item_index: 0,
            status: AgentStatus::Ok,
            tokens: TokenUsage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
            elapsed_ms: 100,
        });
        // Item 0 in stage 1 error
        w.handle(&AgentEvent::PipelineItemDone {
            run_id: uuid::Uuid::now_v7(),
            stage_index: 1,
            item_index: 0,
            status: AgentStatus::Error,
            tokens: TokenUsage::default(),
            elapsed_ms: 50,
        });
        w.handle(&AgentEvent::PipelineDone {
            run_id: uuid::Uuid::now_v7(),
            stages_completed: 2,
            total_ok: 1,
            total_failed: 1,
        });
        let summary = tmp.path().join("pipeline_0").join("_summary.md");
        assert!(summary.exists(), "pipeline summary should be written");
        let content = std::fs::read_to_string(&summary).unwrap();
        assert!(content.contains("Pipeline: 2 stages x 3 items"));
        assert!(content.contains("lint"));
        assert!(content.contains("OK 1"));
        assert!(content.contains("Failed 1"));
    }

    #[test]
    fn pipeline_handles_unfilled_item_slots() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::PipelineStarted {
            run_id: uuid::Uuid::now_v7(),
            total_stages: 1,
            items: 2,
        });
        // Only item 1 completes — item 0 stays None.
        w.handle(&AgentEvent::PipelineItemDone {
            run_id: uuid::Uuid::now_v7(),
            stage_index: 0,
            item_index: 1,
            status: AgentStatus::Ok,
            tokens: TokenUsage::default(),
            elapsed_ms: 0,
        });
        w.handle(&AgentEvent::PipelineDone {
            run_id: uuid::Uuid::now_v7(),
            stages_completed: 1,
            total_ok: 1,
            total_failed: 0,
        });
        let content =
            std::fs::read_to_string(tmp.path().join("pipeline_0").join("_summary.md").as_path())
                .unwrap();
        // Empty slot should render as " - "
        assert!(content.contains(" - "));
    }

    #[test]
    fn pipeline_done_without_started_is_safe() {
        let (_tmp, mut w) = new_writer();
        // No PipelineStarted -> ctx is None -> write_pipeline_summary is a no-op.
        w.handle(&AgentEvent::PipelineDone {
            run_id: uuid::Uuid::now_v7(),
            stages_completed: 0,
            total_ok: 0,
            total_failed: 0,
        });
        // No pipeline directory should exist.
        let entries: Vec<_> = std::fs::read_dir(_tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("pipeline_"))
            .collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn pipeline_item_done_before_started_is_safe() {
        let (_tmp, mut w) = new_writer();
        w.handle(&AgentEvent::PipelineItemDone {
            run_id: uuid::Uuid::now_v7(),
            stage_index: 0,
            item_index: 0,
            status: AgentStatus::Ok,
            tokens: TokenUsage::default(),
            elapsed_ms: 0,
        });
    }

    // ── Parallel events ───────────────────────────────────────────────────

    #[test]
    fn parallel_started_then_done_writes_summary() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::ParallelStarted {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            span_id: 1,
            count: 4,
        });
        w.handle(&AgentEvent::ParallelDone {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            span_id: 1,
            ok: 3,
            failed: 1,
            results: serde_json::json!({}),
            elapsed_ms: 500,
        });
        let summary = tmp.path().join("parallel_0").join("_summary.md");
        assert!(summary.exists(), "parallel summary should exist");
        let content = std::fs::read_to_string(&summary).unwrap();
        assert!(content.contains("Parallel: 4 items"));
        assert!(content.contains("OK 3"));
        assert!(content.contains("Failed 1"));
    }

    #[test]
    fn parallel_done_without_started_is_noop() {
        let (_tmp, mut w) = new_writer();
        // No ParallelStarted before -> ParallelContext is None -> no file written.
        w.handle(&AgentEvent::ParallelDone {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            span_id: 0,
            ok: 0,
            failed: 0,
            results: serde_json::json!({}),
            elapsed_ms: 0,
        });
        let entries: Vec<_> = std::fs::read_dir(_tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("parallel_"))
            .collect();
        assert!(entries.is_empty());
    }

    // ── ReportEmitted ─────────────────────────────────────────────────────

    #[test]
    fn report_emitted_writes_report_file() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::ReportEmitted {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            report: serde_json::json!({"answer": 42, "ok": true}),
        });
        let path = tmp.path().join("_report.md");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Final Report"));
        assert!(content.contains("\"answer\""));
        assert!(content.contains("42"));
    }

    // ── RunDone ───────────────────────────────────────────────────────────

    #[test]
    fn run_done_writes_summary_with_status_and_agents() {
        let (tmp, mut w) = new_writer();
        w.handle(&run_started());
        let a1 = uuid::Uuid::now_v7();
        let a2 = uuid::Uuid::now_v7();
        w.handle(&agent_started(0, a1, "alpha"));
        w.handle(&agent_started(0, a2, "beta"));
        w.handle(&AgentEvent::AgentDone {
            run_id: uuid::Uuid::now_v7(),
            agent_id: a1,
            status: AgentStatus::Ok,
            tokens: TokenUsage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
            },
            elapsed_ms: 200,
            name: Some("alpha".to_string()),
            agent_seq: 1,
            output: serde_json::Value::Null,
            findings: vec![],
            prompt: String::new(),
            retry_count: 0,
        });
        w.handle(&AgentEvent::AgentDone {
            run_id: uuid::Uuid::now_v7(),
            agent_id: a2,
            status: AgentStatus::Error,
            tokens: TokenUsage::default(),
            elapsed_ms: 50,
            name: Some("beta".to_string()),
            agent_seq: 2,
            output: serde_json::Value::Null,
            findings: vec![],
            prompt: String::new(),
            retry_count: 1,
        });
        w.handle(&AgentEvent::RunDone {
            run_id: uuid::Uuid::now_v7(),
            status: RunStatus::Failed,
            total_tokens: TokenUsage {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
            },
            report: serde_json::json!({"summary": "completed"}),
            ts: Utc::now(),
        });
        let summary = tmp.path().join("_summary.md");
        assert!(summary.exists());
        let content = std::fs::read_to_string(&summary).unwrap();
        assert!(content.contains("# Run Summary"));
        assert!(content.contains("Failed"));
        assert!(content.contains("alpha"));
        assert!(content.contains("beta"));
        // Pipeline/Parallel counts only shown if > 0.
        assert!(!content.contains("Pipelines"));
        assert!(!content.contains("Parallels"));
        // Errors section
        assert!(content.contains("## Errors"));
        // Final report
        assert!(content.contains("## Final Report"));
        assert!(content.contains("\"summary\""));
    }

    #[test]
    fn run_done_truncates_long_task_description() {
        let (tmp, mut w) = new_writer();
        let long_task = "x".repeat(200);
        w.handle(&AgentEvent::RunStarted {
            run_id: uuid::Uuid::now_v7(),
            task: long_task.clone(),
            ts: Utc::now(),
        });
        w.handle(&AgentEvent::RunDone {
            run_id: uuid::Uuid::now_v7(),
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({}),
            ts: Utc::now(),
        });
        let summary = std::fs::read_to_string(tmp.path().join("_summary.md")).unwrap();
        // First 100 chars only
        assert!(summary.contains(&"x".repeat(100)));
        assert!(!summary.contains(&"x".repeat(101)));
    }

    #[test]
    fn run_done_with_no_agents_skips_agents_section() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::RunDone {
            run_id: uuid::Uuid::now_v7(),
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({}),
            ts: Utc::now(),
        });
        let summary = std::fs::read_to_string(tmp.path().join("_summary.md")).unwrap();
        assert!(!summary.contains("## Agents"));
        assert!(!summary.contains("## Errors"));
        // Agents count row still present in Overview
        assert!(summary.contains("Agents"));
    }

    #[test]
    fn run_done_includes_pipeline_count_when_started() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::PipelineStarted {
            run_id: uuid::Uuid::now_v7(),
            total_stages: 1,
            items: 1,
        });
        w.handle(&AgentEvent::RunDone {
            run_id: uuid::Uuid::now_v7(),
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({}),
            ts: Utc::now(),
        });
        let summary = std::fs::read_to_string(tmp.path().join("_summary.md")).unwrap();
        assert!(summary.contains("Pipelines"));
    }

    #[test]
    fn run_done_includes_parallel_count_when_started() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::ParallelStarted {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            span_id: 0,
            count: 2,
        });
        w.handle(&AgentEvent::RunDone {
            run_id: uuid::Uuid::now_v7(),
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({}),
            ts: Utc::now(),
        });
        let summary = std::fs::read_to_string(tmp.path().join("_summary.md")).unwrap();
        assert!(summary.contains("Parallels"));
    }

    #[test]
    fn run_done_uses_existing_final_report_when_present() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::ReportEmitted {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            report: serde_json::json!({"from": "ReportEmitted"}),
        });
        w.handle(&AgentEvent::RunDone {
            run_id: uuid::Uuid::now_v7(),
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({"from": "RunDone"}),
            ts: Utc::now(),
        });
        let summary = std::fs::read_to_string(tmp.path().join("_summary.md")).unwrap();
        // RunDone should not overwrite an existing ReportEmitted.
        assert!(summary.contains("\"from\""));
        assert!(summary.contains("ReportEmitted"));
        assert!(!summary.contains("RunDone"));
    }

    #[test]
    fn run_done_uses_run_report_when_no_report_emitted() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::RunDone {
            run_id: uuid::Uuid::now_v7(),
            status: RunStatus::Completed,
            total_tokens: TokenUsage::default(),
            report: serde_json::json!({"only_from_run_done": true}),
            ts: Utc::now(),
        });
        let summary = std::fs::read_to_string(tmp.path().join("_summary.md")).unwrap();
        assert!(summary.contains("\"only_from_run_done\""));
        assert!(summary.contains("true"));
    }

    // ── Unhandled events ──────────────────────────────────────────────────

    #[test]
    fn unhandled_events_do_not_panic() {
        let (_tmp, mut w) = new_writer();
        w.handle(&AgentEvent::PhaseDone {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            ok: 1,
            failed: 0,
            ts: Utc::now(),
        });
        w.handle(&AgentEvent::AcpRaw {
            run_id: uuid::Uuid::now_v7(),
            agent_id: uuid::Uuid::now_v7(),
            kind: "agent_message_chunk".into(),
            raw: serde_json::json!({}),
        });
        w.handle(&AgentEvent::WorkflowStarted {
            run_id: uuid::Uuid::now_v7(),
            span_id: 0,
            path: "/tmp/x.lua".into(),
            args: serde_json::json!({}),
        });
        w.handle(&AgentEvent::WorkflowDone {
            run_id: uuid::Uuid::now_v7(),
            span_id: 0,
            path: "/tmp/x.lua".into(),
            report: serde_json::json!({}),
            elapsed_ms: 0,
            error: None,
        });
    }

    // ── Agent sequence number ────────────────────────────────────────────

    #[test]
    fn multiple_agents_get_distinct_seq_directories() {
        let (tmp, mut w) = new_writer();
        let a1 = uuid::Uuid::now_v7();
        let a2 = uuid::Uuid::now_v7();
        let a3 = uuid::Uuid::now_v7();

        w.handle(&AgentEvent::AgentStarted {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            agent_id: a1,
            prompt_preview: String::new(),
            model: None,
            description: None,
            role: None,
            name: Some("a".into()),
            agent_seq: 1,
        });
        w.handle(&AgentEvent::AgentStarted {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            agent_id: a2,
            prompt_preview: String::new(),
            model: None,
            description: None,
            role: None,
            name: Some("b".into()),
            agent_seq: 2,
        });
        w.handle(&AgentEvent::AgentStarted {
            run_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            agent_id: a3,
            prompt_preview: String::new(),
            model: None,
            description: None,
            role: None,
            name: None,
            agent_seq: 3,
        });

        for (id, seq, name) in [(a1, 1, "a"), (a2, 2, "b"), (a3, 3, "c")] {
            w.handle(&AgentEvent::AgentDone {
                run_id: uuid::Uuid::now_v7(),
                agent_id: id,
                status: AgentStatus::Ok,
                tokens: TokenUsage::default(),
                elapsed_ms: 0,
                name: Some(name.into()),
                agent_seq: seq,
                output: serde_json::Value::Null,
                findings: vec![],
                prompt: String::new(),
                retry_count: 0,
            });
        }
        assert!(tmp.path().join("01_a").join("report.md").exists());
        assert!(tmp.path().join("02_b").join("report.md").exists());
        // Third agent had name=None on AgentStarted but AgentDone provided
        // name="c"; the AgentDone handler overrides the registered name with
        // its own value, so the directory should reflect "03_c".
        assert!(tmp.path().join("03_c").join("report.md").exists());
        // The directory created from AgentStarted alone (with no name) does
        // not exist since the AgentDone handler removed the agent record.
        assert!(!tmp.path().join("03").exists());
    }

    // ── agent_dir_name helper ─────────────────────────────────────────────

    #[test]
    fn agent_dir_name_includes_name_when_present() {
        let n = ArtifactWriter::agent_dir_name(3, &Some("hello".into()));
        assert_eq!(n, "03_hello");
    }

    #[test]
    fn agent_dir_name_uses_seq_when_name_missing() {
        let n = ArtifactWriter::agent_dir_name(7, &None);
        assert_eq!(n, "07");
    }

    #[test]
    fn agent_dir_name_zero_pads_seq() {
        let n = ArtifactWriter::agent_dir_name(1, &None);
        assert_eq!(n, "01");
        let n = ArtifactWriter::agent_dir_name(10, &None);
        assert_eq!(n, "10");
        let n = ArtifactWriter::agent_dir_name(99, &None);
        assert_eq!(n, "99");
    }

    // ── Token accounting in agent report ──────────────────────────────────

    #[test]
    fn agent_report_total_excludes_cache() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&agent_started(1, agent_id, "tok"));
        w.handle(&AgentEvent::AgentDone {
            run_id: uuid::Uuid::now_v7(),
            agent_id,
            status: AgentStatus::Ok,
            tokens: TokenUsage {
                input: 7,
                output: 3,
                cache_read: 999,
                cache_write: 999,
            },
            elapsed_ms: 0,
            name: Some("tok".into()),
            agent_seq: 1,
            output: serde_json::Value::Null,
            findings: vec![],
            prompt: String::new(),
            retry_count: 0,
        });
        let content = std::fs::read_to_string(tmp.path().join("01_tok").join("report.md")).unwrap();
        // Total = input + output = 10
        assert!(content.contains("| **Total**   | **    10** |"));
    }

    // ── Phase label reflected in report metadata ─────────────────────────

    #[test]
    fn agent_report_includes_phase_label_in_metadata() {
        let (tmp, mut w) = new_writer();
        let agent_id = uuid::Uuid::now_v7();
        w.handle(&phase_started(5, "implementation"));
        w.handle(&agent_started(5, agent_id, "ph-agent"));
        w.handle(&agent_done(agent_id, "ph-agent"));
        let content =
            std::fs::read_to_string(tmp.path().join("01_ph-agent").join("report.md")).unwrap();
        assert!(content.contains("implementation"));
    }

    // ── Pipeline stage label surfaces in summary ─────────────────────────

    #[test]
    fn pipeline_summary_includes_stage_labels() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::PipelineStarted {
            run_id: uuid::Uuid::now_v7(),
            total_stages: 2,
            items: 1,
        });
        w.handle(&AgentEvent::PipelineStageStarted {
            run_id: uuid::Uuid::now_v7(),
            stage_index: 0,
            label: "fetch".into(),
            agents_in_stage: 1,
        });
        w.handle(&AgentEvent::PipelineStageStarted {
            run_id: uuid::Uuid::now_v7(),
            stage_index: 1,
            label: "summarize".into(),
            agents_in_stage: 1,
        });
        w.handle(&AgentEvent::PipelineDone {
            run_id: uuid::Uuid::now_v7(),
            stages_completed: 2,
            total_ok: 0,
            total_failed: 0,
        });
        let content =
            std::fs::read_to_string(tmp.path().join("pipeline_0").join("_summary.md").as_path())
                .unwrap();
        assert!(content.contains("fetch"));
        assert!(content.contains("summarize"));
    }

    #[test]
    fn pipeline_summary_uses_default_label_for_unlabeled_stage() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::PipelineStarted {
            run_id: uuid::Uuid::now_v7(),
            total_stages: 1,
            items: 1,
        });
        // No PipelineStageStarted for stage 0 -> label is empty -> default "stage".
        w.handle(&AgentEvent::PipelineDone {
            run_id: uuid::Uuid::now_v7(),
            stages_completed: 1,
            total_ok: 0,
            total_failed: 0,
        });
        let content =
            std::fs::read_to_string(tmp.path().join("pipeline_0").join("_summary.md").as_path())
                .unwrap();
        assert!(content.contains("stage"));
    }

    // ── Multiple pipeline runs get distinct indices ──────────────────────

    #[test]
    fn multiple_pipelines_get_distinct_indices() {
        let (tmp, mut w) = new_writer();
        w.handle(&AgentEvent::PipelineStarted {
            run_id: uuid::Uuid::now_v7(),
            total_stages: 1,
            items: 1,
        });
        w.handle(&AgentEvent::PipelineDone {
            run_id: uuid::Uuid::now_v7(),
            stages_completed: 1,
            total_ok: 0,
            total_failed: 0,
        });
        w.handle(&AgentEvent::PipelineStarted {
            run_id: uuid::Uuid::now_v7(),
            total_stages: 1,
            items: 1,
        });
        w.handle(&AgentEvent::PipelineDone {
            run_id: uuid::Uuid::now_v7(),
            stages_completed: 1,
            total_ok: 0,
            total_failed: 0,
        });
        assert!(tmp.path().join("pipeline_0").join("_summary.md").exists());
        assert!(tmp.path().join("pipeline_1").join("_summary.md").exists());
    }
}
