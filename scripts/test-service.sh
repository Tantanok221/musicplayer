#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

artifact_dir="${SONORA_SERVICE_ARTIFACT_DIR:-target/service-test-artifacts}"
mkdir -p "$artifact_dir"
{
  git rev-parse HEAD
  rustc --version
  cargo --version
  uname -s
} > "$artifact_dir/environment.txt"

# Forward optional test filters/flags, such as a scenario name and --exact.
cargo test --locked --features test-support --test service_worker -- --nocapture "$@" \
  2>&1 | tee "$artifact_dir/test.log"

if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed;' "$artifact_dir/test.log"; then
  echo 'Service test gate ran no passing scenarios.' >&2
  exit 1
fi
