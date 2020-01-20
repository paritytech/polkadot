#!/bin/sh
#
#
# check for any changes in the ^runtime/ tree. if there are no changes check
# if the substrate reference in the Cargo.lock has been changed. If so pull
# the repo and verify if the {spec,impl}_version s have been altered since the
# last reference. If there were changes the script will continue to check if
# the spec_version resp impl_version of polkadot have been altered as well.
# this will also be checked if there were changes to the runtime source files.
#
# If there are any changes found, it will mark the PR breaksapi and
# "auto-fail" the PR if there isn't a change in the
# runtime/{polkadot,kusama}/src/lib.rs file
# that alters the version since the last release tag.


set -e # fail on any error


SUBSTRATE_REPO="https://github.com/paritytech/substrate"
SUBSTRATE_REPO_CARGO="git\+${SUBSTRATE_REPO}\?branch=polkadot-master"
SUBSTRATE_VERSIONS_FILE="bin/node/runtime/src/lib.rs"

boldprint () { printf "|\n| \033[1m${@}\033[0m\n|\n" ; }
boldcat () { printf "|\n"; while read l; do printf "| \033[1m${l}\033[0m\n"; done; printf "|\n" ; }


# figure out the latest release tag
LATEST_TAG="$(git tag -l | sort -V | tail -n 1)"
boldprint "latest release tag ${LATEST_TAG}"


boldprint "latest 10 commits of ${CI_COMMIT_REF_NAME}"
git log --graph --oneline --decorate=short -n 10

boldprint "make sure the master branch is available in shallow clones"
git fetch --depth=${GIT_DEPTH:-100} origin master


github_label () {
	echo
	echo "# run github-api job for labeling it ${1}"
	curl -sS -X POST \
		-F "token=${CI_JOB_TOKEN}" \
		-F "ref=master" \
		-F "variables[LABEL]=${1}" \
		-F "variables[PRNO]=${CI_COMMIT_REF_NAME}" \
		-F "variables[PROJECT]=paritytech/polkadot" \
		${GITLAB_API}/projects/${GITHUB_API_PROJECT}/trigger/pipeline
}


boldprint "check if the wasm sources changed since ${LATEST_TAG}"
if ! git diff --name-only refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} \
	| grep -q -e '^runtime/'
then
	boldprint "no changes to the polkadot runtime source code detected"
	# continue checking if Cargo.lock was updated with a new substrate reference
	# and if that change includes a {spec|impl}_version update.

	SUBSTRATE_REFS_CHANGED="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock \
		| sed -n -r "s~^[\+\-]source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | wc -l)"

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


	SUBSTRATE_PREV_REF="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock \
		| sed -n -r "s~^\-source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | head -n 1)"

	SUBSTRATE_NEW_REF="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock \
		| sed -n -r "s~^\+source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | head -n 1)"


	boldcat <<-EOT
	previous substrate commit id ${SUBSTRATE_PREV_REF}
	new substrate commit id      ${SUBSTRATE_NEW_REF}
	EOT

	# okay so now need to fetch the substrate repository and check whether spec_version or impl_version has changed there
	SUBSTRATE_CLONE_DIR="$(mktemp -t -d substrate-XXX)"
	trap "rm -rf ${SUBSTRATE_CLONE_DIR}" INT QUIT TERM ABRT EXIT


	git clone --branch polkadot-master --depth 100 --no-tags \
	  ${SUBSTRATE_REPO} ${SUBSTRATE_CLONE_DIR}


	# check if there are changes to the spec|impl versions
	git -C ${SUBSTRATE_CLONE_DIR} diff \
		${SUBSTRATE_PREV_REF}..${SUBSTRATE_NEW_REF} ${SUBSTRATE_VERSIONS_FILE} \
		| grep -E '^[\+\-][[:space:]]+(spec|impl)_version: +([0-9]+),$' || exit 0

	boldcat <<-EOT
	spec_version or or impl_version have changed in substrate after updating Cargo.lock
	please make sure versions are bumped in polkadot accordingly
	EOT
fi


# Introduce runtime/polkadot/src/lib.rs once Polkadot mainnet is live.
for VERSIONS_FILE in runtime/kusama/src/lib.rs
do
	# check for spec_version updates: if the spec versions changed, then there is
	# consensus-critical logic that has changed. the runtime wasm blobs must be
	# rebuilt.

	add_spec_version="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
		| sed -n -r "s/^\+[[:space:]]+spec_version: +([0-9]+),$/\1/p")"
	sub_spec_version="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
		| sed -n -r "s/^\-[[:space:]]+spec_version: +([0-9]+),$/\1/p")"


	# see if the version and the binary blob changed
	if [ "${add_spec_version}" != "${sub_spec_version}" ]
	then

		github_label "B2-breaksapi"

		boldcat <<-EOT

			changes to the runtime sources and changes in the spec version.

			spec_version: ${sub_spec_version} -> ${add_spec_version}

		EOT
		continue

	else
		# check for impl_version updates: if only the impl versions changed, we assume
		# there is no consensus-critical logic that has changed.

		add_impl_version="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
			| sed -n -r 's/^\+[[:space:]]+impl_version: +([0-9]+),$/\1/p')"
		sub_impl_version="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
			| sed -n -r 's/^\-[[:space:]]+impl_version: +([0-9]+),$/\1/p')"


		# see if the impl version changed
		if [ "${add_impl_version}" != "${sub_impl_version}" ]
		then
			boldcat <<-EOT

			changes to the runtime sources and changes in the impl version.

			impl_version: ${sub_impl_version} -> ${add_impl_version}

			EOT
			continue
		fi


		boldcat <<-EOT
		wasm source files changed or the spec version in the substrate reference in
		the Cargo.lock but not the spec/impl version. If changes made do not alter
		logic, just bump 'impl_version'. If they do change logic, bump
		'spec_version'.

		source file directories:
		- runtime

		versions file: ${VERSIONS_FILE}

		EOT

		exit 1
	fi
done


exit 0

# vim: noexpandtab
