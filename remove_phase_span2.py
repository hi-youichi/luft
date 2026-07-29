#!/usr/bin/env python3
"""Remove phase span feature from luft codebase - Part 2: Remaining files."""
import re, os

ROOT = r"C:\Users\heycj\dev\luft"

def read(p):
    with open(os.path.join(ROOT, p), 'r', encoding='utf-8') as f:
        return f.read()

def write(p, c):
    with open(os.path.join(ROOT, p), 'w', encoding='utf-8') as f:
        f.write(c)

def edit(p, old, new, label=""):
    c = read(p)
    if old in c:
        c = c.replace(old, new, 1)
        write(p, c)
        print(f"  [OK] {label}")
    else:
        print(f"  [SKIP] {label}")

def edit_all(p, old, new, label=""):
    c = read(p)
    n = c.count(old)
    if n > 0:
        c = c.replace(old, new)
        write(p, c)
        print(f"  [OK] {label} ({n}x)")
    else:
        print(f"  [SKIP] {label}")

def rx(p, pat, label=""):
    c = read(p)
    nc = re.sub(pat, '', c, flags=re.DOTALL)
    if nc != c:
        write(p, nc)
        print(f"  [OK] {label}")
    else:
        print(f"  [SKIP] {label}")

# 8. runtime lib.rs
print("=== runtime lib.rs ===")
P = "crates/luft-runtime/src/lib.rs"
c = read(P)
c = re.sub(r'\r?\n\| `phase_begin\(name\)` .*? \|', '', c)
c = re.sub(r'\r?\n\| `phase_end\(span\)` .*? \|', '', c)
write(P, c)
print("  [OK] Removed phase_begin/phase_end from docs")

# 9. storage writer.rs
print("\n=== storage writer.rs ===")
P = "crates/luft-storage/src/writer.rs"
edit(P, "            AgentEvent::PhaseSpanStarted { .. } | AgentEvent::PhaseSpanDone { .. } => {}\r\n", "", "Remove match arms")
edit(P, '            AgentEvent::PhaseSpanStarted { run_id, .. } => (Some(*run_id), "phase_span_started"),\r\n            AgentEvent::PhaseSpanDone { run_id, .. } => (Some(*run_id), "phase_span_done"),\r\n', "", "Remove from audit_event_type")

# 10. service run.rs
print("\n=== service run.rs ===")
P = "crates/luft-service/src/run.rs"
rx(P, r'\r?\n    // Inject completed phase spans for resume.*?\n    \}\r?\n', "Remove resume injection")
edit_all(P, "            completed_spans: vec![],\r\n", "", "Remove completed_spans")
edit_all(P, "            started_spans: vec![],\r\n", "", "Remove started_spans")

# 11. service query.rs
print("\n=== service query.rs ===")
P = "crates/luft-service/src/query.rs"
edit_all(P, "            completed_spans: vec![],\r\n", "", "Remove completed_spans")
edit_all(P, "            started_spans: vec![],\r\n", "", "Remove started_spans")

# 12. service phases.rs
print("\n=== service phases.rs ===")
P = "crates/luft-service/src/phases.rs"
edit_all(P, "            completed_spans: vec![],\r\n", "", "Remove completed_spans")
edit_all(P, "            started_spans: vec![],\r\n", "", "Remove started_spans")
edit_all(P, "            parent_span_id: None,\r\n", "", "Remove parent_span_id")

# 13. CLI event_log.rs
print("\n=== CLI event_log.rs ===")
P = "crates/luft-cli/src/commands/event_log.rs"
rx(P, r'\r?\n            PhaseSpanStarted \{ span_id, name, depth, \.\.\. \} => \{.*?\n            \}\r?\n', "Remove PhaseSpanStarted display")
rx(P, r'\r?\n            PhaseSpanDone \{ span_id, name, elapsed_ms, \.\.\. \} => \{.*?\n            \}\r?\n', "Remove PhaseSpanDone display")
edit_all(P, "            parent_span_id: None,\r\n", "", "Remove parent_span_id")

# 14. CLI phase_renderer.rs
print("\n=== CLI phase_renderer.rs ===")
P = "crates/luft-cli/src/commands/phase_renderer.rs"
rx(P, r'\r?\n            AgentEvent::PhaseSpanDone \{.*?\n            \}\r?\n', "Remove PhaseSpanDone render")
edit_all(P, "            parent_span_id: None,\r\n", "", "Remove parent_span_id")

# 15. CLI logs.rs
print("\n=== CLI logs.rs ===")
P = "crates/luft-cli/src/commands/logs.rs"
edit_all(P, "            parent_span_id: None,\r\n", "", "Remove parent_span_id")

# 16. CLI artifact_writer.rs
print("\n=== CLI artifact_writer.rs ===")
P = "crates/luft-cli/src/commands/artifact_writer.rs"
edit_all(P, "            parent_span_id: None,\r\n", "", "Remove parent_span_id")

# 17. CLI lua_validate.rs
print("\n=== CLI lua_validate.rs ===")
P = "crates/luft-cli/src/commands/lua_validate.rs"
c = read(P)
# Remove the comment about phase_begin/phase_end
c = re.sub(r'\r?\n// .*?phase_begin.*', '', c)
# Remove the span pairing print line
c = re.sub(r'\r?\n.*?"phase_begin/end paired: \{\}".*?\r?\n', '\n', c)
# Remove the logic that computes span pairing
c = re.sub(r'\r?\n.*?let.*?phase_begin.*?phase_end.*', '', c)
write(P, c)
print("  [OK] Cleaned lua_validate.rs")

# 18. luft builder.rs
print("\n=== luft builder.rs ===")
P = "crates/luft/src/builder.rs"
c = read(P)
c = re.sub(r'\r?\n.*?phase spans.*?', '', c, flags=re.IGNORECASE)
write(P, c)
print("  [OK] Cleaned builder.rs")

# 19. MCP tools.rs
print("\n=== MCP tools.rs ===")
P = "crates/luft-mcp/src/tools.rs"
if os.path.exists(os.path.join(ROOT, P)):
    edit(P, "use crate::state::PhaseSpanSummary;\r\n", "", "Remove PhaseSpanSummary import")
    edit_all(P, "            completed_spans: vec![],\r\n", "", "Remove completed_spans")
    edit_all(P, "            started_spans: vec![],\r\n", "", "Remove started_spans")
    edit_all(P, "            parent_span_id: None,\r\n", "", "Remove parent_span_id")
    rx(P, r'\r?\n.*?completed_spans.*?\r?\n', "Remove completed_spans output line")
    # Remove PhaseSpanSummary from test checkpoint
    rx(P, r'\r?\n            completed_spans: vec!\[PhaseSpanSummary \{.*?\}\],', "Remove PhaseSpanSummary test data")
else:
    print("  [SKIP] tools.rs not found (may be deleted)")

# 20. planner lib.rs
print("\n=== planner lib.rs ===")
P = "crates/luft-planner/src/lib.rs"
c = read(P)
c = c.replace('phase_begin("x")\n', '')
c = c.replace('phase_begin("x")\r\n', '')
c = re.sub(r'\r?\n.*?phase_end\(.*?\)', '', c)
write(P, c)
print("  [OK] Cleaned planner tests")

# 21. Lua files
print("\n=== Lua files ===")
lua_files = [
    "crates/luft-cli/examples/comprehensive-audit.lua",
    "examples/comprehensive-audit.lua",
    "scripts/arch.lua",
    "workflows/codebase-health-audit.lua",
]
for lf in lua_files:
    fp = os.path.join(ROOT, lf)
    if not os.path.exists(fp):
        print(f"  [SKIP] {lf} - not found")
        continue
    c = read(lf)
    # Remove completed_spans references
    c = re.sub(r'\r?\n.*?completed_spans.*?', '', c)
    # Remove phase_begin/phase_end calls
    c = re.sub(r'\r?\n.*?phase_begin\(.*?', '', c)
    c = re.sub(r'\r?\n.*?phase_end\(.*?', '', c)
    write(lf, c)
    print(f"  [OK] {lf}")

print("\n=== Done Part 2 ===")
