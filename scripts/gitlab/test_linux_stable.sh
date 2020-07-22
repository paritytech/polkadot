#!/bin/sh

#shellcheck source=lib.sh
source "$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )/lib.sh"

github_api_polkadot_pull_url="https://api.github.com/repos/paritytech/polkadot/pulls"
# use github api v3 in order to access the data without authentication
github_header="Authorization: token ${GITHUB_PR_TOKEN}"


# either it's a pull request then check for a companion otherwise use
# substrate:master
if expr match "${CI_COMMIT_REF_NAME}" '^[0-9]\+$' >/dev/null
then
  boldprint "this is pull request no ${CI_COMMIT_REF_NAME}"

  pr_data_file="$(mktemp)"
  # get the last reference to a pr in substrate
  curl -sSL -H "${github_header}" -o "${pr_data_file}" \
    "${github_api_polkadot_pull_url}/${CI_COMMIT_REF_NAME}"

  pr_body="$(sed -n -r 's/^[[:space:]]+"body": (".*")[^"]+$/\1/p' "${pr_data_file}")"

  pr_companion="$(echo "${pr_body}" | sed -n -r \
      -e 's;^.*substrate companion: paritytech/substrate#([0-9]+).*$;\1;p' \
      -e 's;^.*substrate companion: https://github.com/paritytech/substrate/pull/([0-9]+).*$;\1;p' \
    | tail -n 1)"

  if [ "${pr_companion}" ]
  then
    echo Substrate path: $SUBSTRATE_PATH
    prepare_git
    prepare_substrate "$pr_companion"
  else
    boldprint "no companion branch found - building your Polkadot branch"
  fi
  rm -f "${pr_data_file}"
else
  boldprint "this is not a pull request - building your Polkadot branch"
fi


# Test Polkadot pr or master branch with this Substrate commit.
time cargo test --all --release --verbose --locked --features runtime-benchmarks
