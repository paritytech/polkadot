#!/usr/bin/env sh

for f in $(find . -name '*.rs'); do
	cat -s $f > $f.temp
	mv $f.temp $f
done
