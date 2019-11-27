#!/bin/sh
#
#
# check for any changes in the runtime tree. if there are no changes run 
# check_substrate which checks if there was an update of the substrate 
# reference in the Cargo.lock. If there were changes the script will continue 
# to check if the spec_version resp impl_version have been altered as well.
# If there are any changes found, it will mark the PR breaksapi and
# "auto-fail" the PR if there isn't a change in the runtime/src/lib.rs file
# that alters the version.
#
# 

set -e # fail on any error

BASE_BRANCH="v0.6"
VERSIONS_FILE="runtime/src/lib.rs"

SUBSTRATE_REPO="https://github.com/paritytech/substrate"
SUBSTRATE_REPO_CARGO="git\+${SUBSTRATE_REPO}\?branch=polkadot-master"
SUBSTRATE_VERSIONS_FILE="node/runtime/src/lib.rs"


# figure out the latest release tag
LATEST_TAG="$(git tag -l "${BASE_BRANCH}.*" | sort -V | tail -n 1)"

# give some context
git log --graph --oneline --decorate=short -n 10



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





# check if the wasm sources changed
if ! git diff --name-only refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} \
	| grep -q -e '^runtime/'
then
	cat <<-EOT
	
	no changes to the runtime source code detected

	EOT

	# continue checking if Cargo.lock was updated with a new substrate reference 
	# and if that change includes a {spec|impl}_version update.

	SUBSTRATE_REFS_CHANGED="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock \
		| sed -n -r "s~^[\+\-]source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | wc -l)"
	
	# check Cargo.lock for substrate ref change
	case "${SUBSTRATE_REFS_CHANGED}" in
	  (0)
	    echo "| substrate refs not changed in Cargo.lock"
	    exit 0
	    ;;
	  (2)
	    echo "|\n| substrate refs updated since ${LATEST_TAG}"
	    ;;
	  (*)
	    echo "| check unsupported: more than one commit targeted in repo ${SUBSTRATE_REPO_CARGO}"
	    exit 1
	esac
	
	
	SUBSTRATE_PREV_REF="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock \
		| sed -n -r "s~^\-source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | head -n 1)"
	
	SUBSTRATE_NEW_REF="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock \
		| sed -n -r "s~^\+source = \"${SUBSTRATE_REPO_CARGO}#([a-f0-9]+)\".*$~\1~p" | sort -u | head -n 1)"
	
	
	cat <<-EOT
	|
	| previous substrate commit id ${SUBSTRATE_PREV_REF}
	| new substrate commit id      ${SUBSTRATE_NEW_REF}
	|
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

	cat <<-EOT
	|
	| spec_version or or impl_version have changed in substrate after updating Cargo.lock
	| please make sure versions are bumped in polkadot accordingly
	|
	EOT
fi



# check for spec_version updates: if the spec versions changed, then there is
# consensus-critical logic that has changed. the runtime wasm blobs must be
# rebuilt.

add_spec_version="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
	| sed -n -r "s/^\+[[:space:]]+spec_version: +([0-9]+),$/\1/p")"
sub_spec_version="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
	| sed -n -r "s/^\-[[:space:]]+spec_version: +([0-9]+),$/\1/p")"


# see if the version changed
if [ "${add_spec_version}" != "${sub_spec_version}" ]
then

	github_label "B2-breaksapi"

	cat <<-EOT
		
		changes to the runtime sources or substrate and changes in the spec version.
	
		spec_version: ${sub_spec_version} -> ${add_spec_version}
	
	EOT
	exit 0

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
		cat <<-EOT
		
		changes to the runtime sources or substrate and changes in the impl version.

		impl_version: ${sub_impl_version} -> ${add_impl_version}

		EOT
		exit 0
	fi


	cat <<-EOT

	wasm source files changed or the spec version in the substrate reference in 
	the Cargo.lock but not the spec/impl version. If changes made do
	not alter logic, just bump 'impl_version'. If they do change logic, bump
	'spec_version'.

	source file directories:
	- runtime

	versions file: ${VERSIONS_FILE}

	EOT

	# drop through and exit 1...
fi

# dropped through. there's something wrong;  exit 1.

exit 1

# vim: noexpandtab
