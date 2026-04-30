#!/usr/bin/env bash
# Phase 8 network-audit helper for the Custom Local LLM Provider (specs/GH9303/).
#
# Usage:
#   ./script/audit_local_provider.sh start         # boot mitmweb on port 8888
#   ./script/audit_local_provider.sh diff capture.flow   # parse a saved flow dump
#                                                  # and report any forbidden hosts
#
# Privacy claim under audit: when a local:* model is selected, zero requests to
# *.warp.dev are produced *for the LLM call itself*. (Non-LLM Warp traffic —
# telemetry, version-check, login flows — is allowed and documented.)

set -euo pipefail

PORT="${MITM_PORT:-8888}"
WEB_PORT="${MITM_WEB_PORT:-8081}"
SAVE_FILE="${SAVE_FILE:-$PWD/local_provider_audit.flow}"

ALLOWED_WARP_PATHS=(
    "/api/auth/.*"
    "/api/version.*"
    "/api/telemetry.*"
    "/api/installation.*"
    "/api/feedback.*"
)
FORBIDDEN_AI_PATHS=(
    "/ai/multi-agent"
    "/agent-mode-evals/.*"
    "/agent-mode-evals/multi-agent"
    "/agent-mode-evals/passive-suggestions"
)

usage() {
    sed -n '2,/^$/p' "$0" | sed 's/^# //; s/^#//'
    exit 1
}

cmd_start() {
    if ! command -v mitmweb >/dev/null 2>&1; then
        echo "ERROR: mitmweb not found. Install with: brew install mitmproxy" >&2
        exit 1
    fi
    echo "Starting mitmweb on port $PORT (web UI on $WEB_PORT)"
    echo "Saving flows to $SAVE_FILE"
    echo
    echo "Now restart Warp with:"
    echo "  HTTPS_PROXY=http://127.0.0.1:$PORT \\"
    echo "  HTTP_PROXY=http://127.0.0.1:$PORT \\"
    echo "  SSL_CERT_FILE=\"\$HOME/.mitmproxy/mitmproxy-ca-cert.pem\" \\"
    echo "  cargo run"
    echo
    echo "When done, hit Ctrl-C, then run:"
    echo "  $0 diff $SAVE_FILE"
    echo
    exec mitmweb \
        --mode regular \
        --listen-port "$PORT" \
        --web-port "$WEB_PORT" \
        --save-stream-file "$SAVE_FILE"
}

cmd_diff() {
    local flow_file="${1:?usage: $0 diff <flow_file>}"
    if ! command -v mitmdump >/dev/null 2>&1; then
        echo "ERROR: mitmdump not found. Install with: brew install mitmproxy" >&2
        exit 1
    fi
    if [[ ! -f "$flow_file" ]]; then
        echo "ERROR: flow file not found: $flow_file" >&2
        exit 1
    fi

    echo "Replaying $flow_file (warp.dev hosts only)..."
    local hits
    hits="$(mitmdump --quiet --no-server -nr "$flow_file" 2>/dev/null \
        | awk '{print $4, $5, $6}' \
        | grep -i 'warp\.dev' \
        || true)"

    if [[ -z "$hits" ]]; then
        echo "No requests to warp.dev observed in this capture."
        return 0
    fi

    echo "warp.dev requests in capture:"
    echo "$hits"
    echo

    local forbidden=0
    while read -r line; do
        for p in "${FORBIDDEN_AI_PATHS[@]}"; do
            if [[ "$line" =~ $p ]]; then
                echo "FORBIDDEN: $line"
                forbidden=$((forbidden + 1))
            fi
        done
    done <<< "$hits"

    if (( forbidden > 0 )); then
        echo
        echo "FAIL: $forbidden forbidden warp.dev path(s) in capture."
        echo "The dispatch fork did not route this request to the local provider."
        return 1
    fi

    echo "PASS: only allowed warp.dev paths in capture (telemetry / version / auth)."
    echo
    echo "If you want to whitelist additional paths, edit ALLOWED_WARP_PATHS at"
    echo "the top of $0."
}

case "${1:-}" in
    start) shift; cmd_start "$@" ;;
    diff)  shift; cmd_diff "$@" ;;
    *)     usage ;;
esac
