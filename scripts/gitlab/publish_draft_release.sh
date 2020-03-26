#!/usr/bin/env bash

# shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

# Set initial variables
substrate_repo="https://github.com/paritytech/substrate"
substrate_dir='./substrate'

# Cloning repos to ensure freshness
echo "[+] Cloning substrate to generate list of changes"
git clone $substrate_repo $substrate_dir
echo "[+] Finished cloning substrate into $substrate_dir"

version="$CI_COMMIT_TAG"
last_version=$(git tag -l | sort -V | grep -B 1 -x "$CI_COMMIT_TAG" | head -n 1)
echo "[+] Version: $version; Previous version: $last_version"

# Check that a signed tag exists on github for this version
echo '[+] Checking tag has been signed'
check_tag "paritytech/polkadot" "$version"
case $? in
  0) echo '[+] Tag found and has been signed'
    ;;
  1) echo '[!] Tag found but has not been signed. Aborting release.'; exit 1
    ;;
  2) echo '[!] Tag not found. Aborting release.'; exit
esac

# Start with referencing current native runtime
# and find any referenced PRs since last release
# Note: Drop any changes that begin with '[contracts]' or 'contracts:'
spec=$(grep spec_version runtime/kusama/src/lib.rs | tail -n 1 | grep -Eo '[0-9]{4}')
echo "[+] Spec version: $spec"
release_text="Native for runtime $spec.

$(sanitised_git_logs "$last_version" "$version" | \
  sed '/^\[contracts\].*/d' | \
  sed '/^contracts:.*/d' \
)"

# Get substrate changes between last polkadot version and current
cur_substrate_commit=$(grep -A 2 'name = "sc-cli"' Cargo.lock | grep -E -o '[a-f0-9]{40}')
git checkout "$last_version"
old_substrate_commit=$(grep -A 2 'name = "sc-cli"' Cargo.lock | grep -E -o '[a-f0-9]{40}')

pushd $substrate_dir || exit
  git checkout polkadot-master > /dev/null
  git pull > /dev/null
  all_substrate_changes="$(sanitised_git_logs "$old_substrate_commit" "$cur_substrate_commit" | sed 's/(#/(paritytech\/substrate#/')"
  substrate_runtime_changes=""
  substrate_api_changes=""
  substrate_client_changes=""
  substrate_changes=""

  echo "[+] Iterating through substrate changes to find labelled PRs"
  while IFS= read -r line; do
    pr_id=$(echo "$line" | sed -E 's/.*#([0-9]+)\)$/\1/')

    # Skip if the PR has the silent label - this allows us to skip a few requests
    if has_label 'paritytech/substrate' "$pr_id" 'B0-silent'; then
      continue
    fi
    if has_label 'paritytech/substrate' "$pr_id" 'B1-runtimenoteworthy'; then
      substrate_runtime_changes="$substrate_runtime_changes
$line"
    fi
    if has_label 'paritytech/substrate' "$pr_id" 'B1-clientnoteworthy'; then
      substrate_client_changes="$substrate_client_changes
$line"
    fi
     if has_label 'paritytech/substrate' "$pr_id" 'B1-apinoteworthy' ; then
      substrate_api_changes="$substrate_api_changes
$line"
      continue
    fi
  done <<< "$all_substrate_changes"
popd || exit

echo "[+] Changes generated. Removing temporary repos"

# Make the substrate section if there are any substrate changes
if [ -n "$substrate_runtime_changes" ] ||
   [ -n "$substrate_api_changes" ] ||
   [ -n "$substrate_client_changes" ]; then
  substrate_changes=$(cat << EOF
Substrate changes
-----------------

EOF
)
  if [ -n "$substrate_runtime_changes" ]; then
    substrate_changes="$substrate_changes

Runtime
-------
$substrate_runtime_changes"
  fi
  if [ -n "$substrate_client_changes" ]; then
    substrate_changes="$substrate_changes

Client
------
$substrate_client_changes"
  fi
  if [ -n "$substrate_api_changes" ]; then
    substrate_changes="$substrate_changes

API
---
$substrate_api_changes"
  fi
  release_text="$release_text

$substrate_changes"
fi

echo "[+] Release text generated: "
echo "$release_text"

echo "[+] Pushing release to github"
# Create release on github
release_name="Kusama $version"
data=$(jq -Rs --arg version "$version" \
  --arg release_name "$release_name" \
  --arg release_text "$release_text" \
'{
  "tag_name": $version,
  "target_commitish": "master",
  "name": $release_name,
  "body": $release_text,
  "draft": true,
  "prerelease": false
}' < /dev/null)

out=$(curl -s -X POST --data "$data" -H "Authorization: token $GITHUB_RELEASE_TOKEN" "$api_base/paritytech/polkadot/releases")

html_url=$(echo "$out" | jq -r .html_url)

if [ "$html_url" == "null" ]
then
  echo "[!] Something went wrong posting:"
  echo "$out"
  # If we couldn't post, don't want to announce in Matrix
  exit 1
else
  echo "[+] Release draft created: $html_url"
fi

echo '[+] Sending draft release URL to Matrix'

msg_body=$(cat <<EOF
**New version of polkadot tagged:** $CI_COMMIT_TAG.
Gav: Draft release created: $html_url
Build pipeline: $CI_PIPELINE_URL
EOF
)
formatted_msg_body=$(cat <<EOF
<strong>New version of polkadot tagged:</strong> $CI_COMMIT_TAG<br />
Gav: Draft release created: $html_url <br />
Build pipeline: $CI_PIPELINE_URL
EOF
)
send_message "$(structure_message "$msg_body" "$formatted_msg_body")" "$MATRIX_ROOM_ID" "$MATRIX_ACCESS_TOKEN"

echo "[+] Done! Maybe the release worked..."
