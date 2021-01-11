#!/usr/bin/env bash
set -e

# This script is to be run when we are happy with a release candidate.
# It accepts a single argument: version, in the format 'v1.2.3'

version="$1"
if [ -z "$version" ]; then
  echo "No version specified, cannot continue"
  exit 1
fi

if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Version should be in the format v1.2.3"
  exit 1
fi

echo '[+] Checking out the release branch'
git checkout release
echo '[+] Pulling latest version of the release branch from github'
git pull
echo '[+] Attempting to merge the release-candidate branch to the release branch'
git merge "$version"
echo '[+] Tagging the release'
git tag -s -m "$version" "$version"
echo '[+] Pushing the release branch and tag to Github. A new release will be created shortly'
git push origin release
git push origin "refs/tags/$version"
