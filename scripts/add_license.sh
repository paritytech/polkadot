#!/usr/bin/env sh

PAT_GPL="^// Copyright.*If not, see <http://www.gnu.org/licenses/>\.$"
PAT_OTHER="^// Copyright"

for f in $(find . -type f | egrep '\.(c|cpp|rs)$'); do
	HEADER=$(head -16 $f)
	if [[ $HEADER =~ $PAT_GPL ]]; then
		BODY=$(tail -n +17 $f)
		cat license_header > temp
		echo "$BODY" >> temp
		mv temp $f
	elif [[ $HEADER =~ $PAT_OTHER ]]; then
		echo "Other license was found do nothing"
	else
		echo "$f was missing header" 
		cat license_header $f > temp
		mv temp $f
	fi
done
