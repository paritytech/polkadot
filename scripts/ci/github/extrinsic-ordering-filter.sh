#!/usr/bin/env bash
# This script is used in a Github Workflow. It helps filtering out what is interesting
# when comparing metadata and spot what would require a tx version bump.

# shellcheck disable=SC2002,SC2086

FILE=$1

# Higlight indexes that were deleted
function find_deletions() {
    echo "\n## Deletions\n"
    RES=$(cat "$FILE" | grep -n '\[\-\]' | tr -s " ")
    if [ "$RES" ]; then
        echo "$RES" | awk '{ printf "%s\\n", $0 }'
    else
        echo "n/a"
    fi
}

# Highlight indexes that have been deleted
function find_index_changes() {
    echo "\n## Index changes\n"
    RES=$(cat "$FILE" | grep -E -n -i 'idx:\s*([0-9]+)\s*(->)\s*([0-9]+)' | tr -s " ")
    if [ "$RES" ]; then
        echo "$RES" | awk '{ printf "%s\\n", $0 }'
    else
        echo "n/a"
    fi
}

# Highlight values that decreased
function find_decreases() {
    echo "\n## Decreases\n"
    OUT=$(cat "$FILE" | grep -E -i -o '([0-9]+)\s*(->)\s*([0-9]+)' | awk '$1 > $3 { printf "%s;", $0 }')
    IFS=$';' LIST=("$OUT")
    unset RES
    for line in "${LIST[@]}"; do
        RES="$RES\n$(cat "$FILE" | grep -E -i -n \"$line\" | tr -s " ")"
    done

    if [ "$RES" ]; then
        echo "$RES" | awk '{ printf "%s\\n", $0 }' | sort -u -g | uniq
    else
        echo "n/a"
    fi
}

echo "\n------------------------------ SUMMARY -------------------------------"
echo "\n⚠️ This filter is here to help spotting changes that should be reviewed carefully."
echo "\n⚠️ It catches only index changes, deletions and value decreases".

find_deletions "$FILE"
find_index_changes "$FILE"
find_decreases "$FILE"
echo "\n----------------------------------------------------------------------\n"
