#!/bin/bash
# Build docker image with cargo-chef
docker build -t paritypr/forrestgump:$1 -f local-build-with-cargo-chef.Dockerfile  ../../../
docker push paritypr/forrestgump:$1


