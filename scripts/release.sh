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
fi

git checkout release
git pull
git merge "$version"
git tag -s -m "$version" "$version"
git push origin release
git push origin "refs/tags/$version"
