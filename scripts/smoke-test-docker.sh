#!/usr/bin/env bash
set -euo pipefail

IMAGE="${1:-aerini-server-smoke}"
PORT=7799
TOKEN="smoke-test-token-0123456789abcdef"
CONTAINER=""
SAVE_BODY_FILE=""

[ -f Dockerfile ] || { echo "Run this from the repo root (Dockerfile not found in $(pwd))."; exit 1; }

cleanup() {
  [ -n "$CONTAINER" ] && docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  [ -n "$SAVE_BODY_FILE" ] && rm -f "$SAVE_BODY_FILE" || true
}
trap cleanup EXIT

fail_with_logs() {
  echo "TIER2: FAIL ($1)"
  if [ -n "$CONTAINER" ]; then
    echo "── container logs ──"
    docker logs "$CONTAINER" 2>&1 || true
  fi
  exit 1
}

echo "building ${IMAGE}..."
docker build -t "$IMAGE" .

echo
echo "── Tier 1: node-bundled runs standalone in the runtime image ──"
NODE_VERSION_OUT="$(docker run --rm --entrypoint /usr/local/bin/node-bundled "$IMAGE" --version)"
echo "node-bundled --version -> ${NODE_VERSION_OUT}"
case "$NODE_VERSION_OUT" in
  v*) echo "TIER1: PASS" ;;
  *)  echo "TIER1: FAIL (unexpected output)"; exit 1 ;;
esac

echo
echo "── Tier 2: Code node execution via the API ──"
CONTAINER="$(docker run -d -p "127.0.0.1:${PORT}:7700" -e AERINI_TOKEN="$TOKEN" "$IMAGE" api --bind 0.0.0.0 --allow-code)"

HEALTHY=0
for _ in $(seq 1 30); do
  curl -fsS "http://127.0.0.1:${PORT}/api/health" >/dev/null 2>&1 && { HEALTHY=1; break; }
  sleep 1
done
[ "$HEALTHY" -eq 1 ] || fail_with_logs "server never became healthy within 30s"

WORKFLOW='{"id":"smoke-test-code-node","name":"smoke test","nodes":[{"id":"n1","node_type_id":"code","node_type":"action","name":"Code","input_schema":{},"output_schema":{},"config":{"code":"output(2 + 2);"}}],"edges":[]}'
SAVE_BODY="$(python3 -c 'import json,sys; print(json.dumps({"workflow_json": sys.argv[1]}))' "$WORKFLOW")"

SAVE_BODY_FILE="$(mktemp "${TMPDIR:-/tmp}/aerini-smoke-save.XXXXXX")"

SAVE_STATUS="$(curl -sS -o "$SAVE_BODY_FILE" -w '%{http_code}' -X POST "http://127.0.0.1:${PORT}/api/workflows" \
  -H "Authorization: Bearer ${TOKEN}" -H "Content-Type: application/json" \
  -d "$SAVE_BODY")" || fail_with_logs "save request could not be sent"
[ "$SAVE_STATUS" = "200" ] || fail_with_logs "save returned HTTP ${SAVE_STATUS}: $(cat "$SAVE_BODY_FILE" 2>/dev/null)"

RUN_RESPONSE="$(curl -fsS -X POST "http://127.0.0.1:${PORT}/api/workflows/smoke-test-code-node/run" \
  -H "Authorization: Bearer ${TOKEN}" -H "Content-Type: application/json" -d '{}')" \
  || fail_with_logs "run request could not be sent"

echo "run response: ${RUN_RESPONSE}"

CHECK="$(echo "$RUN_RESPONSE" | python3 -c '
import json, sys
r = json.load(sys.stdin)
out = r.get("node_outputs", {}).get("n1", {})
if not r.get("success"):
    print("FAIL")
elif out.get("result") == 4:
    print("PASS")
else:
    print("FAIL")
')"
case "$CHECK" in
  FAIL) fail_with_logs "code node did not return the expected result" ;;
  *)    echo "TIER2: ${CHECK}" ;;
esac
exit 0
