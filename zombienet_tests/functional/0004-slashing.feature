Description: Test dispute finality lag when 1/3 of parachain validators always attempt to include an invalid block
Network: ./0004-slashing.toml
Creds: config

honest-validator-0: is up
honest-validator-1: is up
honest-validator-79: is up
malus-validator-0: is up
malus-validator-19: is up

# Check authority status.
honest-validator-0: reports node_roles is 4
honest-validator-1: reports node_roles is 4
honest-validator-79: reports node_roles is 4
malus-validator-0: reports node_roles is 4
malus-validator-19: reports node_roles is 4

# Parachains should be making progress even if we have up to 1/3 malicious validators.
honest-validator-0: parachain 2000 block height is at least 2 within 180 seconds
honest-validator-1: parachain 2001 block height is at least 2 within 180 seconds
honest-validator-79: parachain 2002 block height is at least 2 within 180 seconds

# Check if disputes are concluded in less than 2 blocks.
honest-validator-0: reports polkadot_parachain_disputes_finality_lag is lower than 2
honest-validator-1: reports polkadot_parachain_disputes_finality_lag is lower than 2
honest-validator-79: reports polkadot_parachain_disputes_finality_lag is lower than 2

# Check there is an offence report
honest-validator-0: system event contains "There is an offence reported" within 60 seconds
honest-validator-1: system event contains "There is an offence reported" within 60 seconds
honest-validator-79: system event contains "There is an offence reported" within 60 seconds

# Allow more time for malicious validator activity.
sleep 120 seconds

honest-validator-0: reports parachain_candidate_disputes_total is at least 5 within 15 seconds

# Disputes should always end as "valid"
honest-validator-0: reports parachain_candidate_dispute_concluded{validity="valid"} is at least 5 within 15 seconds
honest-validator-0: reports parachain_candidate_dispute_concluded{validity="invalid"} is 0 within 15 seconds
