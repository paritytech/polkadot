#!/bin/sh
#
# engineering script
# merge with check_runtime.sh when tested and working


SUBSTRATE_REPO="https://github.com/paritytech/substrate\?branch=polkadot-master"
SUBSTRATE_VERSIONS_FILE="node/runtime/src/lib.rs"


check_substrate () {

	SUBSTRATE_REFS_CHANGED="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock | sed -n -r "s~^[\+\-]source = \"git\+${SUBSTRATE_REPO}#([a-f0-9]+)\".*$~\1~p" | sort -u | wc -l)"
	
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
	    echo "| check unsupported: more than one commit targeted in repo ${SUBSTRATE_REPO}"
	    exit 1
	esac
	
	
	SUBSTRATE_PREV_REF="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock | sed -n -r "s~^\-source = \"git\+${SUBSTRATE_REPO}#([a-f0-9]+)\".*$~\1~p" | sort -u | head -n 1)"
	
	SUBSTRATE_NEW_REF="$(git diff refs/tags/${LATEST_TAG}...${CI_COMMIT_SHA} Cargo.lock | sed -n -r "s~^\+source = \"git\+${SUBSTRATE_REPO}#([a-f0-9]+)\".*$~\1~p" | sort -u | head -n 1)"
	
	
	cat <<-EOT
	|
	| previous substrate commit id ${SUBSTRATE_PREV_REF}
	| new substrate commit id      ${SUBSTRATE_NEW_REF}
	|
	EOT

	# okay so now need to fetch the substrate repository and check whether spec_version or impl_version has changed there
	SUBSTRATE_CLONE_DIR="$(mktemp -t -d substrate-XXX)"
	trap "rm -rf ${SUBSTRATE_CLONE_DIR}" INT QUIT TERM ABRT EXIT


	# git clone ${SUBSTRATE_REPO} /tmp/polkadot-check-substrate
	git clone --branch polkadot-master --depth 100 --no-tags \
		file:///home/gabriel/paritytech/source/github.com/paritytech/substrate \
		${SUBSTRATE_CLONE_DIR}


	# TODO don't have the commit that is referenced locally
	SUBSTRATE_PREV_REF=a73793b0e201450fa79c5cfed8e209f47cce925d
	SUBSTRATE_NEW_REF=aa937d9b4e5767f224cf9d5dfbd9a537e97efcfc

	# check if there are changes to the spec|impl versions
	git -C ${SUBSTRATE_CLONE_DIR} diff ${SUBSTRATE_PREV_REF}..${SUBSTRATE_NEW_REF} ${SUBSTRATE_VERSIONS_FILE} | grep -E '^[\+\-][[:space:]]+(spec|impl)_version: +([0-9]+),$' || exit 0

	cat <<-EOT
	|
	| spec_version or or impl_version have changed in substrate after updating Cargo.lock
	| please make sure versions are bumped in polkadot accordingly
	|
	EOT

	# this script will not end the runtime check but instead continue and check if the versions are bumped
	# exit 1
}


# TODO remove
if [ "$1" = "test" ]
then
	CI_COMMIT_SHA="578fcb82088b3f1b5ac5ff2314a9f641128dc316"
	BASE_BRANCH="v0.6"
	LATEST_TAG="$(git tag -l "${BASE_BRANCH}.*" | sort -V | tail -n 1)"

	check_substrate
fi




# vim: noexpandtab
