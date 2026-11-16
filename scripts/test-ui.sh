#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

artifact_dir="${SONORA_UI_ARTIFACT_DIR:-target/ui-test-artifacts}"
mkdir -p "$artifact_dir"
{
  git rev-parse HEAD
  rustc --version
  cargo --version
  uname -s
} > "$artifact_dir/environment.txt"

# pipefail preserves the test failure when tee succeeds. Rust's panic output
# plus the fixture's allowlisted state/geometry/command trace go into the log.
cargo test --locked --features test-support --test ui -- --nocapture "$@" \
  2>&1 | tee "$artifact_dir/test.log"

# A renamed/missing test module must not silently turn this gate green.
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed;' "$artifact_dir/test.log"; then
  echo 'UI test gate ran no passing scenarios.' >&2
  exit 1
fi
