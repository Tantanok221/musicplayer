#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

runs="${1:-20}"
if [[ ! "$runs" =~ ^[1-9][0-9]*$ ]]; then
  echo 'Usage: scripts/test-service-repeat.sh [positive run count]' >&2
  exit 2
fi
artifact_root="${SONORA_SERVICE_REPEAT_ARTIFACT_DIR:-target/service-repeat-artifacts}"
for ((run = 1; run <= runs; run++)); do
  echo "Service integration repetition $run/$runs"
  SONORA_SERVICE_ARTIFACT_DIR="$artifact_root/run-$run" scripts/test-service.sh
done
