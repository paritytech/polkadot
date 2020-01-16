#!/bin/bash

# Function to take 2 git tags/commits and get any lines from commit messages
# that contain something that looks like a PR reference: e.g., (#1234)
sanitised_git_logs(){
  git --no-pager log --pretty=format:"%s" "$1..$2" |
  # Only find messages referencing a PR
  grep -E '\(#[0-9]+\)' |
  # Strip any asterisks
  sed 's/^* //g' |
  # And add them all back
  sed 's/^/* /g'
}

check_tag () {
  tagver=$1
  tag_out=$(curl -H "Authorization: token $GITHUB_RELEASE_TOKEN" -s "$api_base/git/refs/tags/$tagver")
  tag_sha=$(echo "$tag_out" | jq -r .object.sha)
  object_url=$(echo "$tag_out" | jq -r .object.url)
  if [ "$tag_sha" == "null" ]; then
    return 2
  fi
  verified_str=$(curl -H "Authorization: token $GITHUB_RELEASE_TOKEN" -s "$object_url" | jq -r .verification.verified)
  if [ "$verified_str" == "true" ]; then
    # Verified, everything is good
    return 0
  else
    # Not verified. Bad juju.
    return 1
  fi
}

# structure_message $content $formatted_content (optional)
structure_message() {
  if [ -z "$2" ]; then
    body=$(jq -Rs --arg body "$1" '{"msgtype": "m.text", $body}' < /dev/null)
  else
    body=$(jq -Rs --arg body "$1" --arg formatted_body "$2" '{"msgtype": "m.text", $body, "format": "org.matrix.custom.html", $formatted_body}' < /dev/null)
  fi
  echo "$body"
}

# send_message $body (json formatted) $room_id $access_token
send_message() {
curl -XPOST -d "$1" "https://matrix.parity.io/_matrix/client/r0/rooms/$2/send/m.room.message?access_token=$3"
}

# Set initial variables
api_base="https://api.github.com/repos/paritytech/polkadot"
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
check_tag "$version"
case $? in
  0) echo '[+] Tag found and has been signed'
    ;;
  1) echo '[!] Tag found but has not been signed. Aborting release.'; exit 1
    ;;
  2) echo '[!] Tag not found. Aborting release.'; exit 
esac

# Start with referencing current native runtime
# and find any referenced PRs since last release
spec=$(grep spec_version runtime/kusama/src/lib.rs | tail -n 1 | grep -Eo '[0-9]{4}')
echo "[+] Spec version: $spec"
release_text="Native for runtime $spec.

$(sanitised_git_logs "$last_version" "$version")"

# Get substrate changes between last polkadot version and current
cur_substrate_commit=$(grep -A 2 'name = "sc-cli"' Cargo.lock | grep -E -o '[a-f0-9]{40}')
git checkout "$last_version" 2> /dev/null
old_substrate_commit=$(grep -A 2 'name = "sc-cli"' Cargo.lock | grep -E -o '[a-f0-9]{40}')

pushd $substrate_dir || exit
  git checkout polkadot-master > /dev/null
  git pull > /dev/null
  substrate_changes="$(sanitised_git_logs "$old_substrate_commit" "$cur_substrate_commit" | sed 's/(#/(paritytech\/substrate#/')"
popd || exit

echo "[+] Changes generated. Removing temporary repos"
# Should be done with substrate repo now, clean it up
rm -rf $substrate_dir

if [ -n "$substrate_changes" ]; then
  release_text="$release_text

Substrate changes
-----------------
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

out=$(curl -s -X POST --data "$data" -H "Authorization: token $GITHUB_RELEASE_TOKEN" "$api_base/releases")

html_url=$(echo "$out" | jq -r .html_url)

if [ "$html_url" == "null" ]
then
  echo "[!] Something went wrong posting:"
  echo "$out"
else
  echo "[+] Release draft created: $html_url"
fi

echo '[+] Sending draft release URL to Matrix'

msg_body=$(cat <<EOF
**Gav: Release pipeline for Polkadot $version complete.**
Draft release created: $html_url
EOF
)
formatted_msg_body=$(cat <<EOF
<strong>Gav: Release pipeline for Polkadot $version complete.</strong><br />
Draft release created: $html_url
EOF
)
send_message "$(structure_message "$msg_body" "$formatted_msg_body")" "$MATRIX_ROCCESS_TOKEN"

echo "[+] Done! Maybe the release worked..."
