#!/usr/bin/env bash

# Check for any changes in any runtime directories (e.g., ^runtime/polkadot) as
# well as directories common to all runtimes (e.g., ^runtime/common). If there
# are no changes, check if the Substrate git SHA in Cargo.lock has been
# changed. If so, pull the repo and verify if {spec,impl}_versions have been
# altered since the previous Substrate version used. Also, if any of the
# Substrate changes between the previous and current version referenced by
# Cargo.lock were labelled with 'D2-breaksapi', label this PR the same.
#
# If there were changes to any runtimes or common dirs, we iterate over each
# runtime (defined in the $runtimes() array), and check if {spec,impl}_version
# have been changed since the last release. Also, if there have been changes to
# the runtime since the last commit to master, label the PR with 'D2-breaksapi'

set -e # fail on any error

#Include the common functions library
#shellcheck source=../common/lib.sh
. "$(dirname "${0}")/../common/lib.sh"

SUBSTRATE_REPO="https://github.com/paritytech/substrate"
SUBSTRATE_REPO_CARGO="git\+${SUBSTRATE_REPO}"
SUBSTRATE_VERSIONS_FILE="bin/node/runtime/src/lib.rs"

# figure out the latest release tag
boldprint "make sure we have all tags (including those from the release branch)"
git fetch --depth="${GIT_DEPTH:-100}" origin release
git fetch --depth="${GIT_DEPTH:-100}" origin 'refs/tags/*:refs/tags/*'
LATEST_TAG="$(git tag -l | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+-?[0-9]*$' | sort -V | tail -n 1)"
boldprint "latest release tag ${LATEST_TAG}"

boldprint "latest 10 commits of ${CI_COMMIT_REF_NAME}"
git --no-pager log --graph --oneline --decorate=short -n 10

boldprint "make sure the master branch is available in shallow clones"
git fetch --depth="${GIT_DEPTH:-100}" origin master


runtimes=(
  "kusama"
  "polkadot"
  "westend"
)

common_dirs=(
  "common"
)

# Helper function to join elements in an array with a multi-char delimiter
# https://stackoverflow.com/questions/1527049/how-can-i-join-elements-of-an-array-in-bash
function join_by { local d=$1; shift; echo -n "$1"; shift; printf "%s" "${@/#/$d}"; }

# Construct a regex to search for any changes to runtime or common directories
runtime_regex="^runtime/$(join_by '|^runtime/' "${runtimes[@]}" "${common_dirs[@]}")"

boldprint "check if the wasm sources changed since ${LATEST_TAG}"
if ! git diff --name-only "refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA}" \
  | grep -E -q -e "$runtime_regex"
then
  boldprint "no changes to any runtime source code detected"
  # continue checking if Cargo.lock was updated with a new substrate reference
  # and if that change includes a {spec|impl}_version update.

  SUBSTRATE_REFS_CHANGED="$(
    git diff "refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA}" Cargo.lock \
    | sed -n -r "s~^[\+\-]source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | wc -l
  )"

  # check Cargo.lock for substrate ref change
  case "${SUBSTRATE_REFS_CHANGED}" in
    (0)
      boldprint "substrate refs not changed in Cargo.lock"
      exit 0
      ;;
    (2)
      boldprint "substrate refs updated since ${LATEST_TAG}"
      ;;
    (*)
      boldprint "check unsupported: more than one commit targeted in repo ${SUBSTRATE_REPO_CARGO}"
      exit 1
  esac


  SUBSTRATE_PREV_REF="$(
    git diff "refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA}" Cargo.lock \
    | sed -n -r "s~^\-source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | head -n 1
  )"

  SUBSTRATE_NEW_REF="$(
    git diff "refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA}" Cargo.lock \
    | sed -n -r "s~^\+source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | head -n 1
  )"


  boldcat <<EOT
previous substrate commit id ${SUBSTRATE_PREV_REF}
new substrate commit id      ${SUBSTRATE_NEW_REF}
EOT

  # okay so now need to fetch the substrate repository and check whether spec_version or impl_version has changed there
  SUBSTRATE_CLONE_DIR="$(mktemp -t -d substrate-XXXXXX)"
  trap 'rm -rf "${SUBSTRATE_CLONE_DIR}"' INT QUIT TERM ABRT EXIT

  git clone --depth="${GIT_DEPTH:-100}" --no-tags \
    "${SUBSTRATE_REPO}" "${SUBSTRATE_CLONE_DIR}"

  # check if there are changes to the spec|impl versions
  git -C "${SUBSTRATE_CLONE_DIR}" diff \
    "${SUBSTRATE_PREV_REF}..${SUBSTRATE_NEW_REF}" "${SUBSTRATE_VERSIONS_FILE}" \
    | grep -E '^[\+\-][[:space:]]+(spec|impl)_version: +([0-9]+),$' || exit 0

  boldcat <<EOT
spec_version or or impl_version have changed in substrate after updating Cargo.lock
please make sure versions are bumped in polkadot accordingly
EOT

  # Now check if any of the substrate changes have been tagged D2-breaksapi
  (
    cd "${SUBSTRATE_CLONE_DIR}"
    substrate_changes="$(sanitised_git_logs "${SUBSTRATE_PREV_REF}" "${SUBSTRATE_NEW_REF}")"
    echo "$substrate_changes" | while read -r line; do
      pr_id=$(echo "$line" | sed -E 's/.*#([0-9]+)\)$/\1/')

      if has_label 'paritytech/substrate' "$pr_id" 'D2-breaksapi'; then
        boldprint "Substrate change labelled with D2-breaksapi. Labelling..."
        github_label "D2-breaksapi"
        exit 1
      fi
    done
  )

fi

failed_runtime_checks=()

# Iterate over each runtime defined at the start of the script
for RUNTIME in "${runtimes[@]}"
do

  # Check if there were changes to this specific runtime or common directories.
  # If not, we can skip to the next runtime
  regex="^runtime/$(join_by '|^runtime/' "$RUNTIME" "${common_dirs[@]}")"
  if ! git diff --name-only "refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA}" \
    | grep -E -q -e "$regex"; then
    continue
  fi

  # check for spec_version updates: if the spec versions changed, then there is
  # consensus-critical logic that has changed. the runtime wasm blobs must be
  # rebuilt.

  add_spec_version="$(
    git diff "refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA}" "runtime/${RUNTIME}/src/lib.rs" \
    | sed -n -r "s/^\+[[:space:]]+spec_version: +([0-9]+),$/\1/p"
  )"
  sub_spec_version="$(
    git diff "refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA}" "runtime/${RUNTIME}/src/lib.rs" \
    | sed -n -r "s/^\-[[:space:]]+spec_version: +([0-9]+),$/\1/p"
  )"


  # see if the version and the binary blob changed
  if [ "${add_spec_version}" != "${sub_spec_version}" ]
  then

    if git diff --name-only "origin/master...${CI_COMMIT_SHA}" \
      | grep -E -q -e "$regex"
    then
      # add label breaksapi only if this pr altered the runtime sources
      github_label "D2-breaksapi"
    fi

    boldcat <<EOT
## RUNTIME: ${RUNTIME} ##

changes to the ${RUNTIME} runtime sources and changes in the spec version.

spec_version: ${sub_spec_version} -> ${add_spec_version}

EOT
    continue

  else
    # check for impl_version updates: if only the impl versions changed, we assume
    # there is no consensus-critical logic that has changed.

    add_impl_version="$(
      git diff refs/tags/"${LATEST_TAG}...${CI_COMMIT_SHA}" "runtime/${RUNTIME}/src/lib.rs" \
      | sed -n -r 's/^\+[[:space:]]+impl_version: +([0-9]+),$/\1/p'
    )"
    sub_impl_version="$(
      git diff refs/tags/"${LATEST_TAG}...${CI_COMMIT_SHA}" "runtime/${RUNTIME}/src/lib.rs" \
      | sed -n -r 's/^\-[[:space:]]+impl_version: +([0-9]+),$/\1/p'
    )"


    # see if the impl version changed
    if [ "${add_impl_version}" != "${sub_impl_version}" ]
    then
      boldcat <<EOT

## RUNTIME: ${RUNTIME} ##

changes to the ${RUNTIME} runtime sources and changes in the impl version.

impl_version: ${sub_impl_version} -> ${add_impl_version}

EOT
      continue
    fi

    failed_runtime_checks+=("$RUNTIME")
  fi
done

if [ ${#failed_runtime_checks} -gt 0 ]; then
  boldcat <<EOT
wasm source files changed or the spec version in the substrate reference in
the Cargo.lock but not the spec/impl version. If changes made do not alter
logic, just bump 'impl_version'. If they do change logic, bump
'spec_version'.

source file directories:
- runtime

version files: ${failed_runtime_checks[@]}
EOT

  exit 1
fi

exit 0
