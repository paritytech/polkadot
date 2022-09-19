Description: Smoke / Upgrade Node
Network: ./0002-upgrade-node.toml
Creds: config

alice: is up
bob: is up
charlie: is up
dave: is up

alice: parachain 2000 block height is at least 10 within 200 seconds
bob: parachain 2001 block height is at least 10 within 200 seconds

# upgrade both nodes
# For testing using native provider you should set this env var
# POLKADOT_PR_BIN_URL=https://gitlab.parity.io/parity/mirrors/polkadot/-/jobs/1842869/artifacts/raw/artifacts/polkadot
# with the version of polkadot you want to download.

# avg 30s in our infra
alice: run ./0002-download-polkadot-from-pr.sh with "{{POLKADOT_PR_BIN_URL}}" within 40 seconds
bob: run ./0002-download-polkadot-from-pr.sh with "{{POLKADOT_PR_BIN_URL}}" within 40 seconds
alice: restart after 5 seconds
bob: restart after 5 seconds

# process bootstrap
sleep 30 seconds

alice: is up within 10 seconds
bob: is up within 10 seconds


alice: parachain 2000 block height is at least 30 within 300 seconds
bob: parachain 2001 block height is at least 30 within 120 seconds

