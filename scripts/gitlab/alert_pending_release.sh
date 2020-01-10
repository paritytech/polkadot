#!/bin/bash

# structure_message $content $formatted_content (optional)
structure_message() {
  if [ -z "$2" ]; then
    body=$(jq -Rs --arg body "$1" '{"msgtype": "m.text", $body}' < /dev/null)
  else
    body=$(jq -Rs --arg body "$1" --arg formatted_body "$2" '{"msgtype": "m.text", $body, "format": "org.matrix.custom.html", $formatted_body}' < /dev/null)
  fi
  echo $body
}

# send_message $body (json formatted) $room_id $access_token
send_message() {
curl -XPOST -d "$1" "https://matrix.parity.io/_matrix/client/r0/rooms/$2/send/m.room.message?access_token=$3"
}

# Receive keys
trusted_keys=(
27E36F4D3DB8D09946B14802EC077FBE1556877C # gavin@parity.io
)

for key in ${trusted_keys[@]}; do
  gpg --keyserver hkps://keys.openpgp.org --recv-keys $key
done

# If the tag's not signed by any of the above keys, exit failing
if ! git tag -v $CI_COMMIT_TAG; then
  echo "[!] FATAL: TAG NOT VERIFIED WITH A GPG SIGNATURE, QUITTING"
  exit 1
fi

echo "[+] Tag present and verified. Alerting #polkadot and release-manager"

# Format and send message to #polkadot channel
msg_body=$(cat <<EOF
**New version of polkadot tagged:** $CI_COMMIT_TAG.
Build pipeline: $CI_PIPELINE_URL
A release will be created on completion of this pipeline.
EOF
)

# Created formatted body for clients that support it (???)
formatted_msg_body=$(cat <<EOF
<strong>New version of polkadot tagged:</strong> $CI_COMMIT_TAG.<br />
Build pipeline: $CI_PIPELINE_URL<br />
A release will be drafted upon completion of this pipeline.
EOF
)

echo "[+] Sending message to Polkadot room"
send_message "$(structure_message "$msg_body" "$formatted_msg_body")" $MATRIX_ROOM_ID $MATRIX_ACCESS_TOKEN

# Format and send message to release manager
msg_body=$(cat <<EOF
**New version of polkadot tagged:** $CI_COMMIT_TAG.
Build pipeline: $CI_PIPELINE_URL
When the build finishes, it is safe to build cleanroom binaries.
EOF
)

echo "[+] Sending message to release manager"
send_message "$(structure_message "$msg_body")" $REL_MAN_ROOM_ID $MATRIX_ACCESS_TOKEN
