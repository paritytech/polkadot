Description: Test BEEFY voting and finality, test MMR proofs. Assumes Rococo sessions of 1 minute.
Network: ./0003-beefy-and-mmr.toml
Creds: config

# Some sanity checks.
validator-0: is up
validator-1: is up
validator-2: is up
validator-3: is up

# Check authority status.
validator-0: reports node_roles is 4
validator-1: reports node_roles is 4
validator-2: reports node_roles is 4
validator-3: reports node_roles is 4

# BEEFY sanity checks.
validator-0: reports substrate_beefy_validator_set_id is 0
validator-1: reports substrate_beefy_validator_set_id is 0
validator-2: reports substrate_beefy_validator_set_id is 0
validator-3: reports substrate_beefy_validator_set_id is 0

# Verify voting happens and 1st mandatory block is finalized within 1st session.
validator-0: reports substrate_beefy_best_block is at least 1 within 60 seconds
validator-1: reports substrate_beefy_best_block is at least 1 within 60 seconds
validator-2: reports substrate_beefy_best_block is at least 1 within 60 seconds
validator-3: reports substrate_beefy_best_block is at least 1 within 60 seconds

# Pause validator-3 and test chain is making progress without it.
validator-3: pause

# Verify validator sets get changed on new sessions.
validator-0: reports substrate_beefy_validator_set_id is at least 1 within 70 seconds
validator-1: reports substrate_beefy_validator_set_id is at least 1 within 70 seconds
validator-2: reports substrate_beefy_validator_set_id is at least 1 within 70 seconds
# Check next session too.
validator-0: reports substrate_beefy_validator_set_id is at least 2 within 130 seconds
validator-1: reports substrate_beefy_validator_set_id is at least 2 within 130 seconds
validator-2: reports substrate_beefy_validator_set_id is at least 2 within 130 seconds

# Verify voting happens and blocks are being finalized for new sessions too:
# since we verified we're at least in the 3rd session, verify BEEFY finalized mandatory #21.
validator-0: reports substrate_beefy_best_block is at least 21 within 130 seconds
validator-1: reports substrate_beefy_best_block is at least 21 within 130 seconds
validator-2: reports substrate_beefy_best_block is at least 21 within 130 seconds

# TODO (issue #11972): Custom JS to test BEEFY RPCs
# TODO (issue #11972): Custom JS to test MMR RPCs

# Resume validator-3 and verify it imports all BEEFY justification and catches up.
validator-3: resume
validator-3: reports substrate_beefy_validator_set_id is at least 2 within 30 seconds
validator-3: reports substrate_beefy_best_block is at least 21 within 30 seconds
