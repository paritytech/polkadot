#!/usr/bin/env bash

#shellcheck source=../common/lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/../common/lib.sh"

repo="$GITHUB_REPOSITORY"
pr="$GITHUB_PR"

ensure_labels() {
  for label in "$@"; do
    if has_label "$repo" "$pr" "$label"; then
      return 0
    fi
  done
  return 1
}

# Must have one of the following labels
releasenotes_labels=(
  'B0-silent'
  'B1-releasenotes'
  'B7-runtimenoteworthy'
)

priority_labels=(
  'C1-low'
  'C3-medium'
  'C7-high'
  'C9-critical'
)

echo "[+] Checking release notes (B) labels for $CI_COMMIT_BRANCH"
if ensure_labels "${releasenotes_labels[@]}";  then
  echo "[+] Release notes label detected. All is well."
else
  echo "[!] Release notes label not detected. Please add one of: ${releasenotes_labels[*]}"
  exit 1
fi

echo "[+] Checking release priority (C) labels for $CI_COMMIT_BRANCH"
if ensure_labels "${priority_labels[@]}";  then
  echo "[+] Release priority label detected. All is well."
else
  echo "[!] Release priority label not detected. Please add one of: ${priority_labels[*]}"
  exit 1
fi

# If the priority is anything other than C1-low, we *must not* have a B0-silent
# label
if has_label "$repo" "$CI_COMMIT_BRANCH" 'B0-silent' &&
  ! has_label "$repo" "$CI_COMMIT_BRANCH" 'C1-low' ; then
  echo "[!] Changes with a priority higher than C1-low *MUST* have a B- label that is not B0-Silent"
  exit 1
fi

exit 0
