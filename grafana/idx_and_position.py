import json

with open("grafana/dashboard.json") as dash_fp:
    dashboard = json.load(dash_fp)

for (idx, panel) in enumerate(dashboard["panels"]):
    panel["id"] = idx + 1
    panel["gridPos"]["y"] = idx * 8

with open("grafana/dashboard.json", "w") as dash_fp:
    json.dump(dashboard, dash_fp, sort_keys=True, indent=2)
