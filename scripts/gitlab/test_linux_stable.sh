#!/usr/bin/env bash

url="https://api.github.com/repos/paritytech/polkadot/pulls/${CI_COMMIT_REF_NAME}"
echo "[+] API URL: $url"

pr_title=$(curl -H "Authorization: token ${GITHUB_PR_TOKEN}" "$url" | jq -r .title)
echo "[+] PR title: $pr_title"

if echo "$pr_title" | grep -qi '^companion'; then
  echo "[!] PR is a companion PR. Build is already done in substrate"
  exit 0
else
  echo "[+] PR is not a companion PR. Proceeding test"
  time cargo test --all --release --verbose --locked --features runtime-benchmarks
fi
