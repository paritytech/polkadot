Description: Test dispute finality lag at BFT threshold. Malus nodes are configured to back/approve a garbage candidate.
Network: ./0003-parachains-bft-disputes.toml
Creds: config

honest-validator-0: is up
honest-validator-1: is up
honest-validator-2: is up
honest-validator-3: is up
honest-validator-4: is up
honest-validator-5: is up
honest-validator-6: is up
honest-validator-7: is up
honest-validator-8: is up
honest-validator-9: is up
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
honest-validator-7: reports node_roles is 4
honest-validator-8: reports node_roles is 4
honest-validator-9: reports node_roles is 4
malus-validator-0: reports node_roles is 4
malus-validator-1: reports node_roles is 4
malus-validator-2: reports node_roles is 4


honest-validator-0: parachain 2000 block height is at least 4 within 180 seconds
honest-validator-1: parachain 2001 block height is at least 4 within 180 seconds
honest-validator-2: parachain 2002 block height is at least 4 within 180 seconds
honest-validator-3: parachain 2003 block height is at least 4 within 180 seconds

honest-validator-3: log line contains "reverted due to a bad parachain block" within 180 seconds
sleep 60 seconds

honest-validator-0: reports parachain_disputes_finality_lag is lower than 4
honest-validator-1: reports parachain_disputes_finality_lag is lower than 4
honest-validator-2: reports parachain_disputes_finality_lag is lower than 4
honest-validator-3: reports parachain_disputes_finality_lag is lower than 4
honest-validator-6: reports parachain_disputes_finality_lag is lower than 4
honest-validator-7: reports parachain_disputes_finality_lag is lower than 4
honest-validator-8: reports parachain_disputes_finality_lag is lower than 4
honest-validator-9: reports parachain_disputes_finality_lag is lower than 4

honest-validator-0: reports parachain_candidate_disputes_total is at least 4 within 15 seconds
honest-validator-1: reports parachain_candidate_dispute_concluded{validity="invalid"} is at least 4 within 15 seconds
honest-validator-2: reports parachain_candidate_dispute_concluded{validity="valid"} is 0 within 15 seconds
