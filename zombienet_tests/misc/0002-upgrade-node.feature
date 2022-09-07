Description: Smoke / Upgrade Node
Network: ./0002-upgrade-node.toml
Creds: config

alice: is up
bob: is up
charlie: is up
dave: is up

alice: reports block height is at least 10 within 200 seconds
alice: parachain 2000 block height is at least 10 within 200 seconds
bob: reports block height is at least 15 within 240 seconds
bob: parachain 2001 block height is at least 10 within 200 seconds
charlie: reports block height is at least 20 within 320 seconds

# upgrade both nodes
# For testing using native provider you should set this env var
# POLKADOT_PR_BIN_URL=https://gitlab.parity.io/parity/mirrors/polkadot/-/jobs/1810914/artifacts/file/artifacts/polkadot
# with the version of polkadot you want to download.
alice: run ./0002-download-polkadot-from-pr.sh with ["{{POLKADOT_PR_BIN_URL}}"] within 200 seconds
bob: run ./0002-download-polkadot-from-pr.sh with ["{{POLKADOT_PR_BIN_URL}}"] within 200 seconds

alice: reports block height is at least 40 within 200 seconds
bob: reports block height is at least 40 within 200 seconds
alice: parachain 2000 block height is at least 30 within 240 seconds
bob: parachain 2001 block height is at least 30 within 240 seconds
