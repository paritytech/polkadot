Description: Small net without paras does create relay chain blocks
Network: ./0003-small-net-no-paras.toml
Creds: config

alice: is up
bob: is up
charlie: is up
dave: is up
eve: is up
ferdie: is up
one: is up
two: is up

# Check authority status and peers.
alice: reports node_roles is 4
bob: reports node_roles is 4
charlie: reports node_roles is 4
dave: reports node_roles is 4
eve: reports node_roles is 4
ferdie: reports node_roles is 4
one: reports node_roles is 4
two: reports node_roles is 4

alice: reports peers count is at least 7 within 20 seconds
bob: reports peers count is at least 7 within 20 seconds
charlie: reports peers count is at least 7 within 20 seconds
dave: reports peers count is at least 7 within 20 seconds
ferdie: reports peers count is at least 7 within 20 seconds
eve: reports peers count is at least 7 within 20 seconds
one: reports peers count is at least 7 within 20 seconds
two: reports peers count is at least 7 within 20 seconds

alice: reports polkadot_parachain_approval_checking_finality_lag is 0 within 12 seconds
bob: reports polkadot_parachain_approval_checking_finality_lag is 0 within 12 seconds
charlie: reports polkadot_parachain_approval_checking_finality_lag is 0 within 12 seconds
dave: reports polkadot_parachain_approval_checking_finality_lag is 0 within 12 seconds
ferdie: reports polkadot_parachain_approval_checking_finality_lag is 0 within 12 seconds
eve: reports polkadot_parachain_approval_checking_finality_lag is 0 within 12 seconds
one: reports polkadot_parachain_approval_checking_finality_lag is 0 within 12 seconds
two: reports polkadot_parachain_approval_checking_finality_lag is 0 within 12 seconds

# a session covers 3 blocks, so technically `6 seconds/block * 4 blocks` should be sufficient
# to produce 4 blocks, but we add a factor of 2, so 48 seconds
alice: reports block_height{status="best"} is at least 4 within 70 seconds
bob: reports block_height{status="best"} is at least 4 within 70 seconds
charlie: reports block_height{status="best"} is at least 4 within 70 seconds
dave: reports block_height{status="best"} is at least 4 within 70 seconds
ferdie: reports block_height{status="best"} is at least 4 within 70 seconds
eve: reports block_height{status="best"} is at least 4 within 70 seconds
one: reports block_height{status="best"} is at least 4 within 70 seconds
two: reports block_height{status="best"} is at least 4 within 70 seconds

alice: reports block_height{status="finalized"} is at least 4 within 90 seconds
bob: reports block_height{status="finalized"} is at least 4 within 90 seconds
charlie: reports block_height{status="finalized"} is at least 4 within 90 seconds
dave: reports block_height{status="finalized"} is at least 4 within 90 seconds
ferdie: reports block_height{status="finalized"} is at least 4 within 90 seconds
eve: reports block_height{status="finalized"} is at least 4 within 90 seconds
one: reports block_height{status="finalized"} is at least 4 within 90 seconds
two: reports block_height{status="finalized"} is at least 4 within 90 seconds
