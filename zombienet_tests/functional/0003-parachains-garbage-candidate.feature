Description: Test dispute finality lag when 1/3 of parachain validators always attempt to include an invalid block
Network: ./0003-parachains-garbage-candidate.toml
Creds: config

honest-validator-0: is up
honest-validator-1: is up
honest-validator-2: is up
honest-validator-3: is up
honest-validator-4: is up
honest-validator-5: is up
honest-validator-6: is up
malus-validator-0: is up
malus-validator-1: is up
malus-validator-2: is up

# Check authority status.
honest-validator-0: reports node_roles is 4
honest-validator-1: reports node_roles is 4
honest-validator-2: reports node_roles is 4
honest-validator-3: reports node_roles is 4
honest-validator-4: reports node_roles is 4
honest-validator-5: reports node_roles is 4
honest-validator-6: reports node_roles is 4
malus-validator-0: reports node_roles is 4
malus-validator-1: reports node_roles is 4
malus-validator-2: reports node_roles is 4

# Parachains should be making progress even if we have up to 1/3 malicious validators.
honest-validator-0: parachain 2000 block height is at least 2 within 180 seconds
honest-validator-1: parachain 2001 block height is at least 2 within 180 seconds
honest-validator-2: parachain 2002 block height is at least 2 within 180 seconds
honest-validator-3: parachain 2003 block height is at least 2 within 180 seconds
honest-validator-4: parachain 2004 block height is at least 2 within 180 seconds
honest-validator-5: parachain 2005 block height is at least 2 within 180 seconds

# Check for chain reversion after dispute conclusion.
honest-validator-0: log line contains "reverted due to a bad parachain block" within 180 seconds
honest-validator-1: log line contains "reverted due to a bad parachain block" within 180 seconds
honest-validator-2: log line contains "reverted due to a bad parachain block" within 180 seconds
honest-validator-3: log line contains "reverted due to a bad parachain block" within 180 seconds
honest-validator-4: log line contains "reverted due to a bad parachain block" within 180 seconds
honest-validator-5: log line contains "reverted due to a bad parachain block" within 180 seconds
honest-validator-6: log line contains "reverted due to a bad parachain block" within 180 seconds

# Check if disputes are concluded in less than 4 blocks.
honest-validator-0: reports polkadot_parachain_disputes_finality_lag is lower than 4
honest-validator-1: reports polkadot_parachain_disputes_finality_lag is lower than 4
honest-validator-2: reports polkadot_parachain_disputes_finality_lag is lower than 4
honest-validator-3: reports polkadot_parachain_disputes_finality_lag is lower than 4
honest-validator-4: reports polkadot_parachain_disputes_finality_lag is lower than 4
honest-validator-5: reports polkadot_parachain_disputes_finality_lag is lower than 4
honest-validator-6: reports polkadot_parachain_disputes_finality_lag is lower than 4

# Allow more time for malicious validator activity.
sleep 30 seconds

# Check that garbage parachain blocks included by malicious validators are being disputed.
honest-validator-0: reports polkadot_parachain_candidate_disputes_total is at least 2 within 15 seconds
honest-validator-1: reports polkadot_parachain_candidate_disputes_total is at least 2 within 15 seconds
honest-validator-2: reports polkadot_parachain_candidate_disputes_total is at least 2 within 15 seconds
honest-validator-4: reports polkadot_parachain_candidate_disputes_total is at least 2 within 15 seconds
honest-validator-5: reports polkadot_parachain_candidate_disputes_total is at least 2 within 15 seconds
honest-validator-6: reports polkadot_parachain_candidate_disputes_total is at least 2 within 15 seconds

# Disputes should always end as "invalid"
honest-validator-1: reports polkadot_parachain_candidate_dispute_concluded{validity="invalid"} is at least 2 within 15 seconds
honest-validator-2: reports polkadot_parachain_candidate_dispute_concluded{validity="valid"} is 0 within 15 seconds
