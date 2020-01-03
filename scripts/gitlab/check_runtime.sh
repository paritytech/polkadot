#!/bin/sh
#
#
# check for any changes in the ^runtime/ tree. if there are any changes found, 
# it should mark the PR B2-breaksapi and "auto-fail" the PR if there 
# isn't a change in the runtime/{kusama,polkadot}/src/lib.rs file that alters 
# the version.

set -e # fail on any error


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



for VERSIONS_FILE in runtime/kusama/src/lib.rs runtime/polkadot/src/lib.rs
do
	# check if the wasm sources changed
	if ! git diff --name-only origin/master...${CI_COMMIT_SHA} \
		| grep -q -e '^runtime/'
	then
		cat <<-EOT
		
		no changes to the runtime source code detected
	
		EOT
	
		continue
	fi
	
	
	
	# check for spec_version updates: if the spec versions changed, then there is
	# consensus-critical logic that has changed. the runtime wasm blobs must be
	# rebuilt.
	
	add_spec_version="$(git diff origin/master...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
		| sed -n -r "s/^\+[[:space:]]+spec_version: +([0-9]+),$/\1/p")"
	sub_spec_version="$(git diff origin/master...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
		| sed -n -r "s/^\-[[:space:]]+spec_version: +([0-9]+),$/\1/p")"
	
	
	# see if the version and the binary blob changed
	if [ "${add_spec_version}" != "${sub_spec_version}" ]
	then
	
		github_label "B2-breaksapi"
	
		cat <<-EOT
			
			changes to the runtime sources and changes in the spec version.
		
			spec_version: ${sub_spec_version} -> ${add_spec_version}
		
		EOT
		continue
	
	else
		# check for impl_version updates: if only the impl versions changed, we assume
		# there is no consensus-critical logic that has changed.
	
		add_impl_version="$(git diff origin/master...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
			| sed -n -r 's/^\+[[:space:]]+impl_version: +([0-9]+),$/\1/p')"
		sub_impl_version="$(git diff origin/master...${CI_COMMIT_SHA} ${VERSIONS_FILE} \
			| sed -n -r 's/^\-[[:space:]]+impl_version: +([0-9]+),$/\1/p')"
	
	
		# see if the impl version changed
		if [ "${add_impl_version}" != "${sub_impl_version}" ]
		then
			cat <<-EOT
			
			changes to the runtime sources and changes in the impl version.
	
			impl_version: ${sub_impl_version} -> ${add_impl_version}
	
			EOT
			continue
		fi
	
	
		cat <<-EOT
	
		wasm source files changed but not the spec/impl version and the runtime
		binary blob. If changes made do not alter logic, just bump 'impl_version'.
		If they do change logic, bump 'spec_version' and rebuild wasm.
	
		source file directories:
		- runtime
	
		versions file: ${VERSIONS_FILE}
	
		EOT

		exit 1
	fi
done


exit 0

# vim: noexpandtab
