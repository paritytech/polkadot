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
alice: run ./0002-upgrade.sh within 200 seconds
bob: run ./0002-upgrade.sh within 200 seconds
alice: reports block height is at least 40 within 200 seconds
bob: reports block height is at least 40 within 200 seconds
