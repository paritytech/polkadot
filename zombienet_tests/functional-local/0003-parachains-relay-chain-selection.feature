Description: Test BFT threshold with 1/3-1 malus nodes. Malus nodes are configured to dispute all candidates
and fake candidate validation (except during backing). 
Network: ./0003-parachains-relay-chain-selection.toml
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
honest-validator-10: is up
malus-validator-0: is up
malus-validator-1: is up
malus-validator-2: is up
malus-validator-3: is up
malus-validator-4: is up

honest-validator-0: parachain 2000 block height is at least 4 within 120 seconds
honest-validator-1: parachain 2001 block height is at least 4 within 120 seconds
honest-validator-2: parachain 2002 block height is at least 4 within 120 seconds
honest-validator-3: parachain 2003 block height is at least 4 within 120 seconds

# Allow the the chains to make some more progress.
sleep 120 seconds

# A finality lag of 5 sounds pretty bad, but this is the current baseline.
honest-validator-0: reports parachain_disputes_finality_lag is lower than 6
honest-validator-1: reports parachain_disputes_finality_lag is lower than 6 
honest-validator-2: reports parachain_disputes_finality_lag is lower than 6
honest-validator-3: reports parachain_disputes_finality_lag is lower than 6
honest-validator-6: reports parachain_disputes_finality_lag is lower than 6
honest-validator-7: reports parachain_disputes_finality_lag is lower than 6
honest-validator-8: reports parachain_disputes_finality_lag is lower than 6
honest-validator-9: reports parachain_disputes_finality_lag is lower than 6
honest-validator-10: reports parachain_disputes_finality_lag is lower than 6
