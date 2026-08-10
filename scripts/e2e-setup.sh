#!/usr/bin/env bash
set -euo pipefail

# ──────────────────────────────────────────────────────────────────────────────
# Luft Web E2E Test Runner
#   Builds the daemon (if needed), starts it + Vite, runs Playwright tests,
#   and cleans up on exit.
#   Usage: bash scripts/e2e-setup.sh [grep_filter]
# ──────────────────────────────────────────────────────────────────────────────

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WEB_DIR="$ROOT_DIR/web"

DAEMON_PORT=7878
DAEMON_URL="http://127.0.0.1:$DAEMON_PORT"
DAEMON_BIN="target/debug/luft"

VITE_PORT=5173
VITE_URL="http://127.0.0.1:$VITE_PORT"

GREP_FILTER="${1:-}"

# ── Colours for output ───────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Colour

info()  { echo -e "${CYAN}[e2e]${NC} $*"; }
ok()    { echo -e "${GREEN}[e2e]${NC} $*"; }
warn()  { echo -e "${YELLOW}[e2e]${NC} $*"; }
err()   { echo -e "${RED}[e2e]${NC} $*" >&2; }

# ── Background process tracking ───────────────────────────────────────────────
DAEMON_PID=""
VITE_PID=""

cleanup() {
    local exit_code=$?
    info "Cleaning up..."

    if [ -n "$VITE_PID" ] && kill -0 "$VITE_PID" 2>/dev/null; then
        info "Stopping Vite (PID $VITE_PID)..."
        kill "$VITE_PID" 2>/dev/null || true
        wait "$VITE_PID" 2>/dev/null || true
    fi

    if [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        info "Stopping daemon (PID $DAEMON_PID)..."
        # First try graceful stop via the CLI
        "$ROOT_DIR/$DAEMON_BIN" daemon stop 2>/dev/null || true
        # Give it a moment, then force kill if still alive
        sleep 1
        if kill -0 "$DAEMON_PID" 2>/dev/null; then
            kill "$DAEMON_PID" 2>/dev/null || true
            wait "$DAEMON_PID" 2>/dev/null || true
        fi
    fi

    # Also try `luft daemon stop` as a belt-and-suspenders measure
    "$ROOT_DIR/$DAEMON_BIN" daemon stop 2>/dev/null || true

    info "Cleanup complete."
    exit "$exit_code"
}
trap cleanup EXIT INT TERM

# ── 1. Build daemon (if binary doesn't exist) ────────────────────────────────
info "Checking daemon binary..."
if [ -x "$ROOT_DIR/$DAEMON_BIN" ]; then
    ok "Daemon binary already exists at $DAEMON_BIN"
else
    info "Building daemon binary..."
    (cd "$ROOT_DIR" && cargo build --bin luft)
    ok "Daemon binary built at $DAEMON_BIN"
fi

# ── 2. Start daemon ──────────────────────────────────────────────────────────
info "Starting daemon on port $DAEMON_PORT..."
"$ROOT_DIR/$DAEMON_BIN" daemon start --port "$DAEMON_PORT" --foreground &
DAEMON_PID=$!
ok "Daemon started (PID $DAEMON_PID)"

# ── 3. Wait for daemon health endpoint ───────────────────────────────────────
info "Waiting for daemon health endpoint..."
for i in $(seq 1 30); do
    if curl -sf "$DAEMON_URL/api/health" >/dev/null 2>&1; then
        ok "Daemon is healthy ($DAEMON_URL/api/health)"
        break
    fi
    if [ "$i" -eq 30 ]; then
        err "Daemon did not become healthy within 30 seconds"
        exit 1
    fi
    sleep 1
done

# ── 4. Install npm dependencies ──────────────────────────────────────────────
info "Installing npm dependencies..."
(cd "$WEB_DIR" && npm ci)
ok "npm dependencies installed"

# ── 5. Start Vite dev server ─────────────────────────────────────────────────
info "Starting Vite dev server..."
(cd "$WEB_DIR" && npm run dev) &
VITE_PID=$!
ok "Vite started (PID $VITE_PID)"

# ── 6. Wait for Vite ─────────────────────────────────────────────────────────
info "Waiting for Vite dev server..."
for i in $(seq 1 30); do
    if curl -sf "$VITE_URL" >/dev/null 2>&1; then
        ok "Vite dev server is ready ($VITE_URL)"
        break
    fi
    if [ "$i" -eq 30 ]; then
        err "Vite dev server did not start within 30 seconds"
        exit 1
    fi
    sleep 1
done

# ── 7. Install Playwright browsers (if not already installed) ────────────────
if [ ! -d "$HOME/.cache/ms-playwright" ]; then
    info "Installing Playwright browsers..."
    (cd "$WEB_DIR" && npx playwright install chromium)
    ok "Playwright browsers installed"
else
    ok "Playwright browsers already installed"
fi

# ── 8. Run Playwright tests ──────────────────────────────────────────────────
if [ -n "$GREP_FILTER" ]; then
    info "Running Playwright tests (filter: \"$GREP_FILTER\")..."
    (cd "$WEB_DIR" && npx playwright test --grep "$GREP_FILTER")
else
    info "Running all Playwright tests..."
    (cd "$WEB_DIR" && npx playwright test)
fi

TEST_EXIT_CODE=$?
if [ "$TEST_EXIT_CODE" -eq 0 ]; then
    ok "All E2E tests passed!"
else
    err "Some E2E tests failed (exit code: $TEST_EXIT_CODE)"
fi

exit "$TEST_EXIT_CODE"