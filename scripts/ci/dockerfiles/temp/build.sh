#!/usr/bin/env bash

docker build -t parity-xbuilder-aarch64-unknown-linux-gnu -f xbuilder-aarch64-linux-gnu.Dockerfile .

docker images | grep xbuilder
