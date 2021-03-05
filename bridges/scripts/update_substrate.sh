#!/bin/sh

# One-liner to update between Substrate releases
# Usage: ./update_substrate.sh 2.0.0-rc6 2.0.0
set -xeu

OLD_VERSION=$1
NEW_VERSION=$2

find . -type f -name 'Cargo.toml' -exec sed -i '' -e "s/$OLD_VERSION/$NEW_VERSION/g" {} \;
