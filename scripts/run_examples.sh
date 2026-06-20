#!/usr/bin/env bash
# run_examples.sh — Run all Maestro examples with automated assertions.
#
# Usage:
#   bash scripts/run_examples.sh           # run all (mock + opencode)
#   bash scripts/run_examples.sh mock      # mock only (fast)
#   bash scripts/run_examples.sh opencode  # opencode only (needs opencode CLI)
#
# Prerequisites: cargo, jq

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
LOG_DIR="$ROOT_DIR/.maestro/example_logs"
REPORT_DIR="$ROOT_DIR/.maestro/example_reports"
MAESTRO="cargo run --manifest-path $ROOT_DIR/Cargo.toml --"

PASS=0
FAIL=0
SKIP=0
RESULTS=()

# ── helpers ──────────────────────────────────────────────────────────────────

bold()  { printf "\033[1m%s\033[0m\n" "$*"; }
green() { printf "\033[32m  ✓ %s\033[0m\n" "$*"; }
red()   { printf "\033[31m  ✗ %s\033[0m\n" "$*"; }
yellow(){ printf "\033[33m  ⚠ %s\033[0m\n" "$*"; }

assert_exit_0() {
    if [ "$1" -eq 0 ]; then
        green "exit code == 0"
    else
        red "exit code == $1 (expected 0)"
        return 1
    fi
}

assert_jq() {
    local file="$1" filter="$2" expected="$3" label="${4:-$filter}"
    local actual
    actual=$(jq -r "$filter" "$file" 2>/dev/null) || {
        red "$label: jq failed"
        return 1
    }
    if [ "$actual" = "$expected" ]; then
        green "$label == $expected"
    else
        red "$label == $actual (expected $expected)"
        return 1
    fi
}

assert_jq_ge() {
    local file="$1" filter="$2" threshold="$3" label="${4:-$filter}"
    local actual
    actual=$(jq "$filter" "$file" 2>/dev/null) || {
        red "$label: jq failed"
        return 1
    }
    if [ "$actual" -ge "$threshold" ] 2>/dev/null; then
        green "$label >= $threshold (actual: $actual)"
    else
        red "$label == $actual (expected >= $threshold)"
        return 1
    fi
}

assert_file_non_empty() {
    if [ -f "$1" ] && [ -s "$1" ]; then
        green "$(basename "$1") exists and non-empty ($(wc -c < "$1") bytes)"
    else
        red "$(basename "$1") missing or empty"
        return 1
    fi
}

assert_grep_in_file() {
    local file="$1" pattern="$2" label="${3:-$pattern}"
    if grep -q "$pattern" "$file" 2>/dev/null; then
        green "file contains: $label"
    else
        red "file missing: $label"
        return 1
    fi
}

assert_event_span_pairing() {
    local log="$1" started_type="$2" done_type="$3"
    local started done
    started=$(grep -c "\"type\":\"${started_type}\"" "$log" 2>/dev/null || echo 0)
    done=$(grep -c "\"type\":\"${done_type}\"" "$log" 2>/dev/null || echo 0)
    if [ "$started" -eq "$done" ] && [ "$started" -gt 0 ]; then
        green "$started_type/$done_type pairing: $started/$done"
    else
        red "$started_type/$done_type pairing: $started/$done (mismatch)"
        return 1
    fi
}

# Run a single test case. Usage:
#   run_test "name" backend workflow.lua [extra args...]
# Sets $REPORT_JSON to the parsed report JSONL file path.
run_test() {
    local name="$1"; shift
    local backend="$1"; shift
    local workflow="$1"; shift

    local log_file="$LOG_DIR/${name}.jsonl"
    local stdout_file="$LOG_DIR/${name}.stdout"

    bold "[$name] backend=$backend workflow=$(basename "$workflow")"
    mkdir -p "$LOG_DIR" "$REPORT_DIR"

    local rc=0
    $MAESTRO run \
        -w "$workflow" \
        -b "$backend" \
        --headless \
        --log "$log_file" --log-format jsonl \
        "$@" \
        > "$stdout_file" 2>"$LOG_DIR/${name}.stderr" || rc=$?

    if [ $rc -ne 0 ]; then
        red "command failed (exit $rc)"
        cat "$LOG_DIR/${name}.stderr" | head -20
        RESULTS+=("FAIL|$name|exit $rc")
        FAIL=$((FAIL + 1))
        return 0
    fi

    REPORT_JSON="$stdout_file"
    return 0
}

finish_test() {
    local name="$1"
    local ok="$2"
    if [ "$ok" -eq 0 ]; then
        RESULTS+=("PASS|$name")
        PASS=$((PASS + 1))
    else
        RESULTS+=("FAIL|$name|assertions")
        FAIL=$((FAIL + 1))
    fi
}

# ── test cases ───────────────────────────────────────────────────────────────

test_hello_mock() {
    local ok=0
    run_test "hello-mock" mock "$ROOT_DIR/examples/hello.lua" || return 0
    assert_exit_0 0 || ok=$?
    assert_jq "$REPORT_JSON" '.report.status' 'ok' 'status' || ok=$?
    assert_event_span_pairing "$LOG_DIR/hello-mock.jsonl" "agent_started" "agent_done" || ok=$?
    assert_grep_in_file "$LOG_DIR/hello-mock.jsonl" '"type":"report"' 'report event' || ok=$?
    finish_test "hello-mock" "$ok"
}

test_parallel_mock() {
    local ok=0
    run_test "parallel-mock" mock "$ROOT_DIR/examples/parallel-demo.lua" || return 0
    assert_exit_0 0 || ok=$?
    assert_jq "$REPORT_JSON" '.report.total_files' '3' 'total_files' || ok=$?
    assert_jq "$REPORT_JSON" '.report.total_findings' '0' 'total_findings' || ok=$?
    assert_event_span_pairing "$LOG_DIR/parallel-mock.jsonl" "parallel_started" "parallel_done" || ok=$?
    assert_jq "$REPORT_JSON" '[.report.results[] | select(.status == "ok")] | length' '3' 'all_ok' || ok=$?
    finish_test "parallel-mock" "$ok"
}

test_pipeline_mock() {
    local ok=0
    run_test "pipeline-mock" mock "$ROOT_DIR/examples/pipeline-demo.lua" || return 0
    assert_exit_0 0 || ok=$?
    assert_jq "$REPORT_JSON" '.report.ok' '3' 'ok_count' || ok=$?
    assert_jq "$REPORT_JSON" '.report.failed' '0' 'failed_count' || ok=$?
    assert_jq "$REPORT_JSON" '.report.total_stages' '2' 'total_stages' || ok=$?
    assert_event_span_pairing "$LOG_DIR/pipeline-mock.jsonl" "pipeline_started" "pipeline_done" || ok=$?
    finish_test "pipeline-mock" "$ok"
}

test_converge_mock() {
    local ok=0
    run_test "converge-mock" mock "$ROOT_DIR/examples/converge-demo.lua" || return 0
    assert_exit_0 0 || ok=$?
    assert_jq "$REPORT_JSON" '.report.converged' 'true' 'converged' || ok=$?
    assert_jq "$REPORT_JSON" '.report.rounds' '0' 'rounds' || ok=$?
    assert_event_span_pairing "$LOG_DIR/converge-mock.jsonl" "converge_started" "converge_done" || ok=$?
    finish_test "converge-mock" "$ok"
}

test_converge_opencode() {
    if ! command -v opencode &>/dev/null; then
        yellow "skip converge-opencode (opencode not found)"
        SKIP=$((SKIP + 1))
        RESULTS+=("SKIP|converge-opencode")
        return 0
    fi
    local ok=0
    run_test "converge-opencode" opencode "$ROOT_DIR/examples/converge-demo.lua" || return 0
    assert_exit_0 0 || ok=$?
    assert_jq "$REPORT_JSON" '.report.converged' 'true' 'converged' || ok=$?
    assert_jq_ge "$REPORT_JSON" '.report.rounds' 1 'rounds >= 1' || ok=$?
    assert_event_span_pairing "$LOG_DIR/converge-opencode.jsonl" "converge_started" "converge_done" || ok=$?
    finish_test "converge-opencode" "$ok"
}

test_deep_research_opencode() {
    if ! command -v opencode &>/dev/null; then
        yellow "skip deep-research (opencode not found)"
        SKIP=$((SKIP + 1))
        RESULTS+=("SKIP|deep-research")
        return 0
    fi
    local ok=0
    local out="$REPORT_DIR/deep-research.md"
    run_test "deep-research" opencode "$ROOT_DIR/examples/deep-research.lua" -o "$out" || return 0
    assert_exit_0 0 || ok=$?
    assert_jq "$REPORT_JSON" '.report.sub_research_ok' \
        "$(jq '.report.sub_research_total' "$REPORT_JSON")" \
        'all sub-research succeeded' || ok=$?
    assert_file_non_empty "$out" || ok=$?
    assert_grep_in_file "$out" "^# " 'H1 title' || ok=$?
    local h2_count
    h2_count=$(grep -c "^## " "$out" 2>/dev/null || echo 0)
    if [ "$h2_count" -ge 3 ]; then
        green "at least 3 H2 sections (actual: $h2_count)"
    else
        red "fewer than 3 H2 sections (actual: $h2_count)"
        ok=1
    fi
    assert_grep_in_file "$out" "Confidence" 'Confidence section' || ok=$?
    finish_test "deep-research" "$ok"
}

test_architecture_opencode() {
    if ! command -v opencode &>/dev/null; then
        yellow "skip architecture-report (opencode not found)"
        SKIP=$((SKIP + 1))
        RESULTS+=("SKIP|architecture-report")
        return 0
    fi
    local ok=0
    local out="$REPORT_DIR/architecture.md"
    run_test "architecture-report" opencode "$ROOT_DIR/examples/architecture-report.lua" -o "$out" || return 0
    assert_exit_0 0 || ok=$?
    assert_jq "$REPORT_JSON" '.report.successful_analyses' \
        "$(jq '.report.modules_analyzed' "$REPORT_JSON")" \
        'all modules analyzed' || ok=$?
    assert_file_non_empty "$out" || ok=$?
    assert_grep_in_file "$out" "项目概述" '项目概述 section' || ok=$?
    assert_grep_in_file "$out" "整体架构" '整体架构 section' || ok=$?
    assert_grep_in_file "$out" "模块职责" '模块职责 section' || ok=$?
    for mod in core runtime adapters; do
        assert_grep_in_file "$out" "$mod" "module: $mod" || ok=$?
    done
    finish_test "architecture-report" "$ok"
}

# ── main ─────────────────────────────────────────────────────────────────────

MODE="${1:-all}"

mkdir -p "$LOG_DIR" "$REPORT_DIR"

if [ "$MODE" = "mock" ] || [ "$MODE" = "all" ]; then
    bold "\n═══ Level 1: Mock Backend ═══"
    test_hello_mock
    test_parallel_mock
    test_pipeline_mock
    test_converge_mock
fi

if [ "$MODE" = "opencode" ] || [ "$MODE" = "all" ]; then
    bold "\n═══ Level 2: OpenCode Backend ═══"
    test_converge_opencode
    test_deep_research_opencode
    test_architecture_opencode
fi

bold "\n═══ Results ═══"
for r in "${RESULTS[@]}"; do
    IFS='|' read -r status name detail <<< "$r"
    case $status in
        PASS)   green "$name" ;;
        FAIL)   red "$name — $detail" ;;
        SKIP)   yellow "$name" ;;
    esac
done

echo ""
bold "PASS=$PASS  FAIL=$FAIL  SKIP=$SKIP"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
