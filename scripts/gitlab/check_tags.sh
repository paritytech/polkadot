#!/usr/bin/env bash

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

# Must have one of the following labels
labels=(
  'B1-releasenotes'
  'B1-runtimeworthy'
  'B1-silent'
)

for label in "${labels[@]}"; do
  if has_label 'paritytech/polkadot' "$CI_COMMIT_BRANCH" "$label"; then
    echo '[+] Label detected, test passed'
    exit 0
  fi
done

echo "[!] PR does not have one of the required labels! Please add one of: ${labels[*]}"
exit 1
