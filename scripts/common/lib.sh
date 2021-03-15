#!/bin/sh

api_base="https://api.github.com/repos"

# Function to take 2 git tags/commits and get any lines from commit messages
# that contain something that looks like a PR reference: e.g., (#1234)
sanitised_git_logs(){
  git --no-pager log --pretty=format:"%s" "$1...$2" |
  # Only find messages referencing a PR
  grep -E '\(#[0-9]+\)' |
  # Strip any asterisks
  sed 's/^* //g'
}

# Checks whether a tag on github has been verified
# repo: 'organization/repo'
# tagver: 'v1.2.3'
# Usage: check_tag $repo $tagver
check_tag () {
  repo=$1
  tagver=$2
  if [ -n "$GITHUB_RELEASE_TOKEN" ]; then
    echo '[+] Fetching tag using privileged token'
    tag_out=$(curl -H "Authorization: token $GITHUB_RELEASE_TOKEN" -s "$api_base/$repo/git/refs/tags/$tagver")
  else
    echo '[+] Fetching tag using unprivileged token'
    tag_out=$(curl -H "Authorization: token $GITHUB_PR_TOKEN" -s "$api_base/$repo/git/refs/tags/$tagver")
  fi
  tag_sha=$(echo "$tag_out" | jq -r .object.sha)
  object_url=$(echo "$tag_out" | jq -r .object.url)
  if [ "$tag_sha" = "null" ]; then
    return 2
  fi
  echo "[+] Tag object SHA: $tag_sha"
  verified_str=$(curl -H "Authorization: token $GITHUB_RELEASE_TOKEN" -s "$object_url" | jq -r .verification.verified)
  if [ "$verified_str" = "true" ]; then
    # Verified, everything is good
    return 0
  else
    # Not verified. Bad juju.
    return 1
  fi
}

# Checks whether a given PR has a given label.
# repo: 'organization/repo'
# pr_id: 12345
# label: B1-silent
# Usage: has_label $repo $pr_id $label
has_label(){
  repo="$1"
  pr_id="$2"
  label="$3"

  # These will exist if the function is called in Gitlab.
  # If the function's called in Github, we should have GITHUB_ACCESS_TOKEN set
  # already.
  if [ -n "$GITHUB_RELEASE_TOKEN" ]; then
    GITHUB_TOKEN="$GITHUB_RELEASE_TOKEN"
  elif [ -n "$GITHUB_PR_TOKEN" ]; then
    GITHUB_TOKEN="$GITHUB_PR_TOKEN"
  fi

  out=$(curl -H "Authorization: token $GITHUB_TOKEN" -s "$api_base/$repo/pulls/$pr_id")
  [ -n "$(echo "$out" | tr -d '\r\n' | jq ".labels | .[] | select(.name==\"$label\")")" ]
}

github_label () {
  echo
  echo "# run github-api job for labeling it ${1}"
  curl -sS -X POST \
    -F "token=${CI_JOB_TOKEN}" \
    -F "ref=master" \
    -F "variables[LABEL]=${1}" \
    -F "variables[PRNO]=${CI_COMMIT_REF_NAME}" \
    -F "variables[PROJECT]=paritytech/polkadot" \
    "${GITLAB_API}/projects/${GITHUB_API_PROJECT}/trigger/pipeline"
}

# Formats a message into a JSON string for posting to Matrix
# message: 'any plaintext message'
# formatted_message: '<strong>optional message formatted in <em>html</em></strong>'
# Usage: structure_message $content $formatted_content (optional)
structure_message() {
  if [ -z "$2" ]; then
    body=$(jq -Rs --arg body "$1" '{"msgtype": "m.text", $body}' < /dev/null)
  else
    body=$(jq -Rs --arg body "$1" --arg formatted_body "$2" '{"msgtype": "m.text", $body, "format": "org.matrix.custom.html", $formatted_body}' < /dev/null)
  fi
  echo "$body"
}

# Post a message to a matrix room
# body: '{body: "JSON string produced by structure_message"}'
# room_id: !fsfSRjgjBWEWffws:matrix.parity.io
# access_token: see https://matrix.org/docs/guides/client-server-api/
# Usage: send_message $body (json formatted) $room_id $access_token
send_message() {
curl -XPOST -d "$1" "https://matrix.parity.io/_matrix/client/r0/rooms/$2/send/m.room.message?access_token=$3"
}

# Pretty-printing functions
boldprint () { printf "|\n| \033[1m%s\033[0m\n|\n" "${@}"; }
boldcat () { printf "|\n"; while read -r l; do printf "| \033[1m%s\033[0m\n" "${l}"; done; printf "|\n" ; }

skip_if_companion_pr() {
  url="https://api.github.com/repos/paritytech/polkadot/pulls/${CI_COMMIT_REF_NAME}"
  echo "[+] API URL: $url"

  pr_title=$(curl -sSL -H "Authorization: token ${GITHUB_PR_TOKEN}" "$url" | jq -r .title)
  echo "[+] PR title: $pr_title"

  if echo "$pr_title" | grep -qi '^companion'; then
    echo "[!] PR is a companion PR. Build is already done in substrate"
    exit 0
  else
    echo "[+] PR is not a companion PR. Proceeding test"
  fi
}

# Fetches the tag name of the latest release from a repository
# repo: 'organisation/repo'
# Usage: latest_release 'paritytech/polkadot'
latest_release() {
  curl -s "$api_base/$1/releases/latest" | jq -r '.tag_name'
}
