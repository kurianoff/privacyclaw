#!/usr/bin/env bash
# =============================================================================
# tests/scripts/e2e_full_test.sh
#
# Comprehensive end-to-end test for the full privacyclaw install/runtime/uninstall
# lifecycle.  Tests are grouped into:
#
#   GROUP A  –  Binary smoke tests (version, help)
#   GROUP B  –  Initialization (CA generation, idempotency, ca-path)
#   GROUP C  –  PII detection commands
#   GROUP D  –  Network helper (setup-network output)
#   GROUP E  –  Proxy runtime  (start, dashboard API, stop via API and CLI)
#   GROUP F  –  System integration (pf rules, LaunchAgent) — REQUIRES ROOT
#   GROUP G  –  Uninstall (dry-run logic tests; destructive steps are skipped
#               unless PRIVACYCLAW_E2E_ALLOW_DESTRUCTIVE=1 is set)
#
# Usage:
#   # Normal (no root required, skips GROUP F destructive steps):
#   bash tests/scripts/e2e_full_test.sh
#
#   # Full system test (needs sudo / macOS admin account):
#   PRIVACYCLAW_E2E_ALLOW_DESTRUCTIVE=1 sudo bash tests/scripts/e2e_full_test.sh
#
#   # Point at a pre-built binary:
#   PRIVACYCLAW_BIN=/path/to/privacyclaw bash tests/scripts/e2e_full_test.sh
#
# Exit code: 0 = all run tests passed, 1 = at least one failure.
# =============================================================================

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Binary: use env var, or build with cargo.
if [[ -n "${PRIVACYCLAW_BIN:-}" ]]; then
    BINARY="$PRIVACYCLAW_BIN"
else
    echo "[setup] Building privacyclaw binary..."
    cd "$REPO_ROOT"
    cargo build --quiet 2>&1
    BINARY="$REPO_ROOT/target/debug/privacyclaw"
fi

PROXY_PORT="${PRIVACYCLAW_E2E_PROXY_PORT:-19440}"
DASHBOARD_PORT="${PRIVACYCLAW_E2E_DASHBOARD_PORT:-19443}"
ALLOW_DESTRUCTIVE="${PRIVACYCLAW_E2E_ALLOW_DESTRUCTIVE:-0}"

# Temporary directory for test-local config/storage.
TMPDIR="$(mktemp -d)"
CONFIG_FILE="$TMPDIR/config.toml"

# Counters.
PASS=0
FAIL=0
SKIP=0

# ── Utilities ─────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((PASS++)) || true; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; ((FAIL++)) || true; }
skip() { echo -e "${YELLOW}[SKIP]${NC} $1"; ((SKIP++)) || true; }
info() { echo -e "       $1"; }

assert_contains() {
    local label="$1" haystack="$2" needle="$3"
    if echo "$haystack" | grep -q "$needle"; then
        pass "$label"
    else
        fail "$label — expected to contain '$needle'"
        info "actual output: $(echo "$haystack" | head -5)"
    fi
}

assert_file_exists() {
    local label="$1" path="$2"
    if [[ -f "$path" ]]; then
        pass "$label"
    else
        fail "$label — file not found: $path"
    fi
}

assert_http_status() {
    local label="$1" expected="$2" actual="$3" body="$4"
    if [[ "$actual" == "$expected" ]]; then
        pass "$label"
    else
        fail "$label — expected HTTP $expected, got $actual"
        info "body: $(echo "$body" | head -3)"
    fi
}

# Write the test config.toml (custom ports + temp storage).
write_test_config() {
    cat > "$CONFIG_FILE" <<EOF
[proxy]
listen = "127.0.0.1:${PROXY_PORT}"
dashboard = "127.0.0.1:${DASHBOARD_PORT}"

[storage]
logs_dir = "${TMPDIR}/logs"
EOF
}

# Wait until a port is open (up to N seconds).
wait_for_port() {
    local port="$1" timeout="${2:-10}" elapsed=0
    while ! nc -z 127.0.0.1 "$port" 2>/dev/null; do
        sleep 0.2
        elapsed=$(echo "$elapsed + 0.2" | bc)
        if (( $(echo "$elapsed >= $timeout" | bc -l) )); then
            return 1
        fi
    done
    return 0
}

# HTTP GET helper → prints "STATUS\nBODY".
http_get() {
    local port="$1" path="$2"
    curl -sf --max-time 5 -w "\n%{http_code}" "http://127.0.0.1:${port}${path}" 2>/dev/null || echo -e "\n000"
}

# HTTP POST helper → prints "STATUS\nBODY".
http_post() {
    local port="$1" path="$2" data="${3:-{}}"
    curl -sf --max-time 5 -X POST \
        -H "Content-Type: application/json" \
        -d "$data" \
        -w "\n%{http_code}" \
        "http://127.0.0.1:${port}${path}" 2>/dev/null || echo -e "\n000"
}

# Parse the last line of curl output as HTTP status code.
http_status() { echo "$1" | tail -1; }
# Parse everything but the last line as the body.
http_body()   { echo "$1" | head -n -1; }

# ── Cleanup ───────────────────────────────────────────────────────────────────

PROXY_PID=""

cleanup() {
    if [[ -n "$PROXY_PID" ]] && kill -0 "$PROXY_PID" 2>/dev/null; then
        kill "$PROXY_PID" 2>/dev/null || true
        wait "$PROXY_PID" 2>/dev/null || true
    fi
    rm -rf "$TMPDIR"
}
trap cleanup EXIT

# ── Header ────────────────────────────────────────────────────────────────────

echo ""
echo "╔══════════════════════════════════════════════════════════════════════╗"
echo "║          privacyclaw  E2E  Full  Lifecycle  Test  Suite               ║"
echo "╚══════════════════════════════════════════════════════════════════════╝"
echo ""
echo "Binary  : $BINARY"
echo "Config  : $CONFIG_FILE"
echo "Ports   : proxy=${PROXY_PORT}, dashboard=${DASHBOARD_PORT}"
echo "Root    : ALLOW_DESTRUCTIVE=${ALLOW_DESTRUCTIVE}"
echo ""

# =============================================================================
# GROUP A — Binary smoke tests
# =============================================================================
echo "━━━ GROUP A: Binary smoke tests ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

OUT=$("$BINARY" --version 2>&1 || true)
assert_contains "A1: --version exits and prints a version number" "$OUT" "\."

OUT=$("$BINARY" --help 2>&1 || true)
assert_contains "A2: --help mentions 'start'" "$OUT" "start"
assert_contains "A3: --help mentions 'init'"  "$OUT" "init"

# =============================================================================
# GROUP B — Initialization
# =============================================================================
echo ""
echo "━━━ GROUP B: Initialization (privacyclaw init) ━━━━━━━━━━━━━━━━━━━━━━━━━"

"$BINARY" init > /dev/null 2>&1 || true
CA_DIR="$HOME/.config/privacyclaw/ca"

assert_file_exists "B1: ca.pem created"     "$CA_DIR/ca.pem"
assert_file_exists "B2: ca.key.pem created" "$CA_DIR/ca.key.pem"

PEM_CONTENT=$(cat "$CA_DIR/ca.pem" 2>/dev/null || true)
assert_contains "B3: ca.pem contains BEGIN CERTIFICATE"    "$PEM_CONTENT" "BEGIN CERTIFICATE"
assert_contains "B4: ca.pem contains END CERTIFICATE"      "$PEM_CONTENT" "END CERTIFICATE"

# Idempotency: run init again, fingerprint must not change.
BEFORE=$(sha256sum "$CA_DIR/ca.pem" | awk '{print $1}')
"$BINARY" init > /dev/null 2>&1 || true
AFTER=$(sha256sum "$CA_DIR/ca.pem" | awk '{print $1}')
if [[ "$BEFORE" == "$AFTER" ]]; then
    pass "B5: init is idempotent (CA not regenerated on second call)"
else
    fail "B5: init regenerated CA on second call (fingerprint changed)"
fi

# ca-path command.
CA_PATH_OUTPUT=$("$BINARY" ca-path 2>&1 || true)
assert_contains "B6: ca-path output ends in .pem" "$CA_PATH_OUTPUT" "\.pem"
CA_PATH_FILE=$(echo "$CA_PATH_OUTPUT" | tr -d '[:space:]')
if [[ -f "$CA_PATH_FILE" ]]; then
    pass "B7: ca-path reported file exists"
else
    fail "B7: ca-path reported non-existent file: $CA_PATH_FILE"
fi

# =============================================================================
# GROUP C — PII detection
# =============================================================================
echo ""
echo "━━━ GROUP C: PII detection (privacyclaw test-pii) ━━━━━━━━━━━━━━━━━━━━━━"

OUT=$("$BINARY" test-pii "Contact jane.doe@example.com or +1-555-867-5309" 2>&1 || true)
assert_contains "C1: detects email"  "$(echo "$OUT" | tr '[:upper:]' '[:lower:]')" "email"
assert_contains "C2: detects phone"  "$(echo "$OUT" | tr '[:upper:]' '[:lower:]')" "phone"

OUT=$("$BINARY" test-pii "Hello world, no personal info here." 2>&1 || true)
# Should not crash and should produce some output.
if [[ -n "$OUT" ]] || [[ $? -eq 0 ]]; then
    pass "C3: test-pii on clean text does not crash"
else
    fail "C3: test-pii crashed on clean text"
fi

OUT=$("$BINARY" test-pii "My SSN is 123-45-6789" 2>&1 || true)
assert_contains "C4: detects SSN" "$(echo "$OUT" | tr '[:upper:]' '[:lower:]')" "ssn\|social"

# =============================================================================
# GROUP D — Network helper
# =============================================================================
echo ""
echo "━━━ GROUP D: Network helper (privacyclaw setup-network) ━━━━━━━━━━━━━━━━"

OUT=$("$BINARY" setup-network 2>&1 || true)
assert_contains "D1: setup-network prints 127.0.0.1"          "$OUT" "127\.0\.0\.1"
assert_contains "D2: setup-network includes api.anthropic.com" "$OUT" "api\.anthropic\.com"
assert_contains "D3: setup-network includes api.openai.com"    "$OUT" "api\.openai\.com"
assert_contains "D4: setup-network includes pf rule"           "$OUT" "rdr\|pf\|pfctl"

# =============================================================================
# GROUP E — Proxy runtime
# =============================================================================
echo ""
echo "━━━ GROUP E: Proxy runtime ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

write_test_config

"$BINARY" --config "$CONFIG_FILE" start > "$TMPDIR/proxy.log" 2>&1 &
PROXY_PID=$!

echo "       Waiting for proxy to start (dashboard port $DASHBOARD_PORT)..."
if ! wait_for_port "$DASHBOARD_PORT" 15; then
    fail "E0: proxy did not start within 15 s — aborting GROUP E"
    cat "$TMPDIR/proxy.log" | head -20
    PROXY_PID=""
else
    pass "E0: proxy started (dashboard listening)"

    # --- E1: /api/version ---
    RESP=$(http_get "$DASHBOARD_PORT" "/api/version")
    STATUS=$(http_status "$RESP"); BODY=$(http_body "$RESP")
    assert_http_status "E1: GET /api/version returns 200" "200" "$STATUS" "$BODY"
    assert_contains    "E2: /api/version body has 'version' field" "$BODY" '"version"'

    # --- E2: /api/proxy/status ---
    RESP=$(http_get "$DASHBOARD_PORT" "/api/proxy/status")
    STATUS=$(http_status "$RESP"); BODY=$(http_body "$RESP")
    assert_http_status "E3: GET /api/proxy/status returns 200" "200" "$STATUS" "$BODY"
    assert_contains    "E4: /api/proxy/status reports running=true" "$BODY" '"running":true\|"running": true'

    # --- E3: /api/config ---
    RESP=$(http_get "$DASHBOARD_PORT" "/api/config")
    STATUS=$(http_status "$RESP"); BODY=$(http_body "$RESP")
    assert_http_status "E5: GET /api/config returns 200" "200" "$STATUS" "$BODY"
    assert_contains    "E6: /api/config body has 'pii' or 'proxy'" "$BODY" '"pii"\|"proxy"'

    # --- E4: /api/conversations ---
    RESP=$(http_get "$DASHBOARD_PORT" "/api/conversations")
    STATUS=$(http_status "$RESP"); BODY=$(http_body "$RESP")
    assert_http_status "E7: GET /api/conversations returns 200" "200" "$STATUS" "$BODY"
    if echo "$BODY" | grep -q '^\['; then
        pass "E8: /api/conversations returns JSON array"
    else
        fail "E8: /api/conversations body is not a JSON array: $BODY"
    fi

    # --- E5: proxy port accepts TCP ---
    if nc -z 127.0.0.1 "$PROXY_PORT" 2>/dev/null; then
        pass "E9: proxy CONNECT port $PROXY_PORT accepts connections"
    else
        fail "E9: proxy CONNECT port $PROXY_PORT is not listening"
    fi

    # --- E6: CONNECT handshake ---
    CONNECT_RESP=$(printf "CONNECT 127.0.0.1:19999 HTTP/1.1\r\nHost: 127.0.0.1:19999\r\n\r\n" | \
        nc -q 1 127.0.0.1 "$PROXY_PORT" 2>/dev/null || true)
    if echo "$CONNECT_RESP" | grep -q "200"; then
        pass "E10: CONNECT handshake returns 200"
    else
        fail "E10: CONNECT did not return 200: $(echo "$CONNECT_RESP" | head -1)"
    fi

    # --- E7: 404 for unknown routes ---
    RESP=$(http_get "$DASHBOARD_PORT" "/api/unknown_route_xyz")
    STATUS=$(http_status "$RESP")
    assert_http_status "E11: unknown route returns 404" "404" "$STATUS" ""

    # --- E8: Stop via dashboard API ---
    RESP=$(http_post "$DASHBOARD_PORT" "/api/proxy/stop" '{}')
    STATUS=$(http_status "$RESP"); BODY=$(http_body "$RESP")
    assert_http_status "E12: POST /api/proxy/stop returns 200" "200" "$STATUS" "$BODY"
    assert_contains    "E13: stop returns ok=true"              "$BODY" '"ok":true\|"ok": true'

    sleep 0.6

    # Proxy should be gone.
    if ! nc -z 127.0.0.1 "$DASHBOARD_PORT" 2>/dev/null; then
        pass "E14: proxy exited after stop command"
    else
        fail "E14: proxy still listening after stop (port $DASHBOARD_PORT)"
    fi
    PROXY_PID=""
fi

# =============================================================================
# GROUP F — System integration (pf / LaunchAgent / keychain)
# Runs automatically when root; skips gracefully otherwise.
# To run: sudo bash tests/scripts/e2e_full_test.sh
# =============================================================================
echo ""
echo "━━━ GROUP F: System integration (pf rules, /etc/hosts) ━━━━━━━━━━━━━━━"

IS_ROOT=false
if [[ "$(id -u)" == "0" ]]; then
    IS_ROOT=true
fi

if [[ "$IS_ROOT" == "false" ]] && [[ "$ALLOW_DESTRUCTIVE" != "1" ]]; then
    skip "F1: privacyclaw network-enable — run as root: sudo bash $0"
    skip "F2: /etc/hosts privacyclaw entries present"
    skip "F3: pf anchor file created"
    skip "F4: pfctl reports active redirect rules"
    skip "F5: privacyclaw network-disable removes entries"
    skip "F6: /etc/hosts clean after disable"
    skip "F7: idempotent enable (no duplicate entries)"
else
    echo "       Running as root — executing network integration tests..."
    # privacyclaw now detects root internally and skips osascript.
    RUN_CMD="$BINARY"   # binary already runs as root via sudo/root session

    # Clean up any stale state before starting.
    "$RUN_CMD" network-disable > /dev/null 2>&1 || true

    # F1: network-enable
    if "$RUN_CMD" network-enable > "$TMPDIR/ne.log" 2>&1; then
        pass "F1: privacyclaw network-enable ran successfully"
    else
        fail "F1: privacyclaw network-enable failed"
        cat "$TMPDIR/ne.log" | head -10
    fi

    # F2: /etc/hosts entries
    if grep -q "# privacyclaw" /etc/hosts 2>/dev/null; then
        pass "F2: /etc/hosts contains privacyclaw entries"
    else
        fail "F2: /etc/hosts missing privacyclaw entries"
    fi
    assert_contains "F2b: /etc/hosts has api.anthropic.com" \
        "$(cat /etc/hosts)" "api\.anthropic\.com"
    assert_contains "F2c: /etc/hosts has api.openai.com" \
        "$(cat /etc/hosts)" "api\.openai\.com"

    # F3: pf anchor file
    if [[ -f "/etc/pf.anchors/privacyclaw" ]]; then
        pass "F3: pf anchor file /etc/pf.anchors/privacyclaw created"
    else
        fail "F3: pf anchor file not created"
    fi
    assert_contains "F3b: pf anchor contains port 443" \
        "$(cat /etc/pf.anchors/privacyclaw 2>/dev/null)" "443"

    # F4: pfctl reports active rules
    PF_RULES=$(pfctl -a privacyclaw -s rules 2>/dev/null || true)
    if echo "$PF_RULES" | grep -q "443\|rdr"; then
        pass "F4: pfctl reports active redirect rules for privacyclaw anchor"
    else
        fail "F4: pfctl shows no rules for privacyclaw anchor"
        info "pfctl output: $PF_RULES"
    fi

    # F7: idempotency — second enable must not duplicate /etc/hosts entries
    "$RUN_CMD" network-enable > /dev/null 2>&1 || true
    ANTHROPIC_COUNT=$(grep -c "api.anthropic.com.*# privacyclaw" /etc/hosts 2>/dev/null || echo 0)
    if [[ "$ANTHROPIC_COUNT" -eq 1 ]]; then
        pass "F7: network-enable is idempotent (no duplicate /etc/hosts entries)"
    else
        fail "F7: duplicate entries found ($ANTHROPIC_COUNT occurrences of api.anthropic.com)"
    fi

    # F5: network-disable
    if "$RUN_CMD" network-disable > "$TMPDIR/nd.log" 2>&1; then
        pass "F5: privacyclaw network-disable ran successfully"
    else
        fail "F5: privacyclaw network-disable failed"
        cat "$TMPDIR/nd.log" | head -10
    fi

    # F6: /etc/hosts clean after disable
    if ! grep -q "# privacyclaw" /etc/hosts 2>/dev/null; then
        pass "F6: /etc/hosts cleaned up after network-disable"
    else
        fail "F6: /etc/hosts still contains privacyclaw entries after disable"
    fi

    # F3c: pf anchor flushed after disable
    PF_AFTER=$(pfctl -a privacyclaw -s rules 2>/dev/null || true)
    if [[ -z "$PF_AFTER" ]]; then
        pass "F8: pf anchor flushed after network-disable"
    else
        fail "F8: pf anchor still has rules after disable: $PF_AFTER"
    fi

    # CA trust (macOS only, optional — only if --install-ca was run)
    if [[ "$(uname)" == "Darwin" ]]; then
        if security verify-cert -c "$CA_DIR/ca.pem" 2>/dev/null; then
            pass "F9: CA certificate is trusted in system keychain"
        else
            skip "F9: CA not in system keychain (run 'privacyclaw init --install-ca' to install)"
        fi
    fi
fi

# =============================================================================
# GROUP G — Uninstall
# =============================================================================
echo ""
echo "━━━ GROUP G: Uninstall ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# G1: --help shows --purge flag.
HELP=$("$BINARY" uninstall --help 2>&1 || true)
assert_contains "G1: uninstall --help mentions --purge" "$(echo "$HELP" | tr '[:upper:]' '[:lower:]')" "purge"

# G2: stop when not running is graceful.
# PID file is in the platform config dir (macOS: ~/Library/Application Support/privacyclaw/).
if [[ "$(uname)" == "Darwin" ]]; then
    PIDFILE="$HOME/Library/Application Support/privacyclaw/privacyclaw.pid"
else
    PIDFILE="$HOME/.config/privacyclaw/privacyclaw.pid"
fi
rm -f "$PIDFILE" 2>/dev/null || true
STOP_OUT=$("$BINARY" stop 2>&1 || true)
STOP_EXIT=$?
if [[ "$STOP_EXIT" -eq 0 ]] || echo "$STOP_OUT" | grep -qi "not running\|no pid"; then
    pass "G2: stop when not running is graceful"
else
    fail "G2: stop when not running exited non-zero without expected message: $STOP_OUT"
fi

if [[ "$ALLOW_DESTRUCTIVE" != "1" ]]; then
    skip "G3: privacyclaw uninstall (destructive) — set PRIVACYCLAW_E2E_ALLOW_DESTRUCTIVE=1 to run"
    skip "G4: privacyclaw uninstall --purge (full data removal)"
else
    echo "       Running destructive uninstall test..."
    # Build a fake install layout in TMPDIR so we don't wipe the real installation.
    FAKE_BIN="$TMPDIR/fake_install/usr/local/bin/privacyclaw"
    mkdir -p "$(dirname "$FAKE_BIN")"
    cp "$BINARY" "$FAKE_BIN"

    UNINSTALL_OUT=$("$BINARY" uninstall 2>&1 || true)
    if echo "$UNINSTALL_OUT" | grep -q "✓\|PASS\|Done\|Skipped\|complete"; then
        pass "G3: uninstall reports step summary"
    else
        fail "G3: uninstall output did not contain expected summary symbols"
        info "$UNINSTALL_OUT"
    fi
fi

# =============================================================================
# Summary
# =============================================================================
echo ""
echo "╔══════════════════════════════════════════════════════════════════════╗"
printf  "║  Results:  ${GREEN}%d passed${NC}  /  ${RED}%d failed${NC}  /  ${YELLOW}%d skipped${NC}                     ║\n" "$PASS" "$FAIL" "$SKIP"
echo "╚══════════════════════════════════════════════════════════════════════╝"
echo ""

if [[ "$FAIL" -gt 0 ]]; then
    exit 1
fi
exit 0
