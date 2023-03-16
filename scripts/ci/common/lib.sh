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

# Check for runtime changes between two commits. This is defined as any changes
# to /primitives/src/* and any *production* chains under /runtime
has_runtime_changes() {
  from=$1
  to=$2

  if git diff --name-only "${from}...${to}" \
    | grep -q -e '^runtime/polkadot' -e '^runtime/kusama' -e '^primitives/src/' -e '^runtime/common'
  then
    return 0
  else
    return 1
  fi
}

# given a bootnode and the path to a chainspec file, this function will create a new chainspec file
# with only the bootnode specified and test whether that bootnode provides peers
# The optional third argument is the index of the bootnode in the list of bootnodes, this is just used to pick an ephemeral
# port for the node to run on. If you're only testing one, it'll just use the first ephemeral port
# BOOTNODE: /dns/polkadot-connect-0.parity.io/tcp/443/wss/p2p/12D3KooWEPmjoRpDSUuiTjvyNDd8fejZ9eNWH5bE965nyBMDrB4o
# CHAINSPEC_FILE: /path/to/polkadot.json
check_bootnode(){
    BOOTNODE=$1
    BASE_CHAINSPEC=$2
    RUNTIME=$(basename "$BASE_CHAINSPEC" | cut -d '.' -f 1)
    MIN_PEERS=1

    # Generate a temporary chainspec file containing only the bootnode we care about
    TMP_CHAINSPEC_FILE="$RUNTIME.$(echo "$BOOTNODE" | tr '/' '_').tmp.json"
    jq ".bootNodes = [\"$BOOTNODE\"] " < "$CHAINSPEC_FILE" > "$TMP_CHAINSPEC_FILE"

    # Grab an unused port by binding to port 0 and then immediately closing the socket
    # This is a bit of a hack, but it's the only way to do it in the shell
    RPC_PORT=$(python -c "import socket; s=socket.socket(); s.bind(('', 0)); print(s.getsockname()[1]); s.close()")

    echo "[+] Checking bootnode $BOOTNODE"
    polkadot --chain "$TMP_CHAINSPEC_FILE" --no-mdns --rpc-port="$RPC_PORT" --tmp > /dev/null 2>&1 &
    # Wait a few seconds for the node to start up
    sleep 5
    POLKADOT_PID=$!

    MAX_POLLS=10
    TIME_BETWEEN_POLLS=3
    for _ in $(seq 1 "$MAX_POLLS"); do
    # Check the health endpoint of the RPC node
      PEERS="$(curl -s -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"system_health","params":[],"id":1}' http://localhost:"$RPC_PORT" | jq -r '.result.peers')"
      # Sometimes due to machine load or other reasons, we don't get a response from the RPC node
      # If $PEERS is an empty variable, make it 0 so we can still do the comparison
      if [ -z "$PEERS" ]; then
        PEERS=0
      fi
      if [ "$PEERS" -ge $MIN_PEERS ]; then
        echo "[+] $PEERS peers found for $BOOTNODE"
        echo "    Bootnode appears contactable"
        kill $POLKADOT_PID
        # Delete the temporary chainspec file now we're done running the node
        rm "$TMP_CHAINSPEC_FILE"
        return 0
      fi
      sleep "$TIME_BETWEEN_POLLS"
    done
    kill $POLKADOT_PID
    # Delete the temporary chainspec file now we're done running the node
    rm "$TMP_CHAINSPEC_FILE"
    echo "[!] No peers found for $BOOTNODE"
    echo "    Bootnode appears unreachable"
    return 1
}
