#!/bin/bash

ROOT="$(dirname "$0")/../../.."
RUNTIME="$1"

# If we're on a mac, use gdate for date command (requires coreutils installed via brew)
if [[ "$OSTYPE" == "darwin"* ]]; then
  DATE="gdate"
else
  DATE="date"
fi

function check_date() {
  # Get the dates as input arguments
  LAST_RUN="$1"
  TODAY="$($DATE +%Y-%m-%d)"
  # Calculate the date two days before today
  CUTOFF=$($DATE -d "$TODAY - 2 days" +%Y-%m-%d)

  if [[ "$LAST_RUN" > "$CUTOFF" ]]; then
    return 0
  else
    return 1
  fi
}

check_weights(){
    FILE=$1
    CUR_DATE=$2
    DATE_REGEX='[0-9]{4}-[0-9]{2}-[0-9]{2}'
    LAST_UPDATE="$(grep -E "//! DATE: $DATE_REGEX" "$FILE" | sed -r "s/.*DATE: ($DATE_REGEX).*/\1/")"
    # If the file does not contain a date, flag it as an error.
    if [ -z "$LAST_UPDATE" ]; then
        echo "Skipping $FILE, no date found."
        return 0
    fi
    if ! check_date "$LAST_UPDATE" ; then
        echo "ERROR: $FILE was not updated for the current date. Last update: $LAST_UPDATE"
        return 1
    fi
    # echo "OK: $FILE"
}

echo "Checking weights for $RUNTIME"
CUR_DATE="$(date +%Y-%m-%d)"
HAS_ERROR=0
for FILE in "$ROOT"/runtime/"$RUNTIME"/src/weights/*.rs; do
    if ! check_weights "$FILE" "$CUR_DATE"; then
        HAS_ERROR=1
    fi
done

if [ $HAS_ERROR -eq 1 ]; then
    echo "ERROR: One or more weights files were not updated during the last benchmark run. Check the logs above."
    exit 1
fi