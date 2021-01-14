#!/usr/bin/env bash

set -e

dashboard="$(<grafana/dashboard-template.json)"

idx="0"
for result in $(rg -trust 'parachain_.*_total' -oN); do
  file="$(cut -d':' -f1 <<< "$result")"
  subsystem="$(cut -d'/' -f2- <<< "$file")"
  subsystem="$(rev <<< "$subsystem" | cut -d'/' -f3- | rev)"

  total="$(cut -d':' -f2 <<< "$result")"
  # when computing the name, we don't want to expand $1; it's got special meaning for ripgrep
  #shellcheck disable=SC2016
  name="$(rg 'parachain_(.*)_total' --replace='$1' <<< "$total")"

  title="$(printf "Relay %s: %s / Second" "$subsystem" "$name")"
  expr="$(printf 'sum by (instance) (rate(polkadot_%s{job=\\"nodes-rococo\\"}[10m]))' "$total")"

  y="$((idx * 8))"
  idx="$((idx+1))"

  panel="$(
    jq ".title = \"$title\"" grafana/panel-template.json |
    jq ".targets[0].expr = \"$expr\"" |
    jq ".gridPos.y = $y"
  )"

  dashboard="$(jq ".panels += [$panel]" <<< "$dashboard")"
done

jq . "$dashboard"
