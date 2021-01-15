#!/usr/bin/env bash
set -e

temp="$(mktemp)"

function apply_expr() {
  expr="$1"

  jq "$expr" grafana/dashboard.json > "$temp"
  cp "$temp" grafana/dashboard.json
}

cp grafana/dashboard-template.json grafana/dashboard.json
apply_expr '.title = "Polkadot Subsystem Timers"'

# the replace expression uses a tab character to separate the two fields, because
#  1. It's unlikely to conflict with any character within the fields, and
#  2. It's the `cut` command's default separator.
#
# However, there isn't a great way to write a tab literal within bash, so let's just
# create it as a variable here
#
# Also, we want not to expand these now, but within ripgrep; single quotes are appropriate
#shellcheck disable=SC2016
replace_expr="$(printf '$1\t$2')"

# we don't want to expand $1; it's got special meaning for ripgrep
#shellcheck disable=SC2016
for result in $(rg '\.(\w+)\.start_timer\(\)' -trust --replace='$1' -oN); do
  file="$(cut -d':' -f1 <<< "$result")"
  subsystem="$(cut -d'/' -f2- <<< "$file")"
  subsystem="$(rev <<< "$subsystem" | cut -d'/' -f3- | rev)"

  timer_field="$(cut -d':' -f2 <<< "$result")"

  # inject the field name with printf so we don't have to mess as much with quote escaping
  regex="$(printf '%s: prometheus::register\(.*?\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,?\s*\)' "$timer_field")"

  # find the registration function so we can figure out what this is called within prometheus
  match="$(
    rg \
    --multiline --multiline-dotall \
    -oN \
    --replace "$replace_expr" \
    "$regex" \
    "$file"
  )"
  prometheus_name="$(cut -f 1 <<< "$match")"
  expr="$(printf "sum by (le) (polkadot_%s_bucket)" "$prometheus_name")"
  docs="$(cut -f 2 <<< "$match")"

  if [ -z "$prometheus_name" ]; then
    echo "WARN: failed to find registration for $timer_field; skipping"
    continue
  fi

  panel="$(
    jq ".title = \"$prometheus_name timer\"" grafana/heatmap-panel-template.json |
    jq ".targets[0].expr = \"$expr\"" |
    jq ".description = \"$docs\""
  )"

  apply_expr ".panels += [$panel]"
done

apply_expr '.panels |= sort_by(.title)'

# fall back to python to compute the id and index of each panel from the sorted list;
# if that's possible in jq, it's complicated
python3 grafana/idx_and_position.py

echo "ok; output in grafana/dashboard.json"
