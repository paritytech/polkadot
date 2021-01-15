#!/usr/bin/env bash

set -e

temp="$(mktemp)"

function apply_expr() {
  expr="$1"

  jq "$expr" grafana/dashboard.json > "$temp"
  cp "$temp" grafana/dashboard.json
}

cp grafana/dashboard-template.json grafana/dashboard.json
apply_expr '.title = "Polkadot Subsystem Counter Rates"'

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

  panel="$(
    jq ".title = \"$title\"" grafana/panel-template.json |
    jq ".targets[0].expr = \"$expr\""
  )"

  apply_expr ".panels += [$panel]"
done

apply_expr '.panels |= sort_by(.title)'

# fall back to python to compute the id and index of each panel from the sorted list;
# if that's possible in jq, it's complicated
python3 grafana/idx_and_position.py

echo "ok; output in grafana/dashboard.json"
