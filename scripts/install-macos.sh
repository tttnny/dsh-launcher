#!/usr/bin/env bash
# Native prerequisites on macOS: Xcode CLT is already installed on hosted
# runners; Rust targets are added by the workflow toolchain step.
set -euo pipefail

echo "dsh-launcher CI: macOS prerequisites are preinstalled on the hosted runner."