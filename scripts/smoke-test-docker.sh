#!/usr/bin/env bash
set -euo pipefail

IMAGE="${1:-aerini-server-smoke}"
PORT=7799
TOKEN="smoke-test-token-0123456789abcdef"
CONTAINER=""
SAVE_BODY_FILE=""
SERVE_CONFIG_FILE=""
SERVE_VOLUME=""

[ -f Dockerfile ] || { echo "Run this from the repo root (Dockerfile not found in $(pwd))."; exit 1; }

# shellcheck disable=SC2317,SC2329 # invoked through the EXIT trap
cleanup() {
  if [ -n "$CONTAINER" ]; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  fi
  if [ -n "$SAVE_BODY_FILE" ]; then
    rm -f "$SAVE_BODY_FILE" || true
  fi
  if [ -n "$SERVE_CONFIG_FILE" ]; then
    rm -f "$SERVE_CONFIG_FILE" || true
  fi
  if [ -n "$SERVE_VOLUME" ]; then
    docker volume rm -f "$SERVE_VOLUME" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

fail_with_logs() {
  echo "SMOKE: FAIL ($1)"
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

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
CONTAINER=""

echo
echo "── Tier 3: serve mode answers health and reports healthy to Docker ──"
SERVE_CONFIG_FILE="$(mktemp "${TMPDIR:-/tmp}/aerini-smoke-serve.XXXXXX")"
SERVE_VOLUME="aerini-smoke-serve-$$"
python3 - "$SERVE_CONFIG_FILE" <<'PY'
import json, sys
workflow = {
    "id": "smoke-test-serve",
    "name": "smoke serve",
    "nodes": [{
        "id": "n1", "node_type_id": "schedule", "node_type": "action",
        "name": "Schedule", "input_schema": {}, "output_schema": {},
        "config": {"mode": "interval", "interval_secs": 3600},
    }],
    "edges": [],
}
config = {
    "workflow_name": "smoke serve",
    "workflow_json": json.dumps(workflow),
    "status_port": 7700,
    "exported_at": "2026-01-01T00:00:00Z",
    "credential_env_vars": {},
}
with open(sys.argv[1], "w") as f:
    json.dump(config, f)
PY
chmod 644 "$SERVE_CONFIG_FILE"
docker volume create "$SERVE_VOLUME" >/dev/null

CONTAINER="$(docker run -d -p "127.0.0.1:${PORT}:7700" \
  -v "${SERVE_VOLUME}:/data" \
  -v "${SERVE_CONFIG_FILE}:/data/aerini-server.json:ro" \
  "$IMAGE" serve --config /data/aerini-server.json --bind 0.0.0.0)"

SERVE_UP=0
for _ in $(seq 1 30); do
  curl -fsS "http://127.0.0.1:${PORT}/api/health" >/dev/null 2>&1 && { SERVE_UP=1; break; }
  sleep 1
done
[ "$SERVE_UP" -eq 1 ] || fail_with_logs "serve mode never answered /api/health within 30s"

curl -fsS "http://127.0.0.1:${PORT}/api/status" >/dev/null 2>&1 \
  || fail_with_logs "serve mode /api/status did not answer"
curl -fsS "http://127.0.0.1:${PORT}/" >/dev/null 2>&1 \
  || fail_with_logs "serve mode status page did not answer"

# The image's HEALTHCHECK waits 10s, then probes every 30s; allow for two probes.
DOCKER_HEALTH=""
for _ in $(seq 1 90); do
  DOCKER_HEALTH="$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{end}}' "$CONTAINER" 2>/dev/null || true)"
  [ "$DOCKER_HEALTH" = "healthy" ] && break
  sleep 1
done
[ "$DOCKER_HEALTH" = "healthy" ] || fail_with_logs "docker health status is '${DOCKER_HEALTH}', expected 'healthy'"
echo "TIER3: PASS"
exit 0
