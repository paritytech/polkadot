Description: Disputes initiation, conclusion and lag
Network: ./0002-parachains-disputes.toml
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

# Ensure parachains are registered.
alice: parachain 2000 is registered within 30 seconds
bob: parachain 2001 is registered within 30 seconds
charlie: parachain 2002 is registered within 30 seconds
dave: parachain 2003 is registered within 30 seconds

alice: reports peers count is at least 11 within 20 seconds
bob: reports peers count is at least 11 within 20 seconds
charlie: reports peers count is at least 11 within 20 seconds
dave: reports peers count is at least 11 within 20 seconds
ferdie: reports peers count is at least 1 within 20 seconds
eve: reports peers count is at least 11 within 20 seconds
one: reports peers count is at least 11 within 20 seconds
two: reports peers count is at least 11 within 20 seconds

# Ensure parachains made progress.
alice: parachain 2000 block height is at least 10 within 200 seconds
alice: parachain 2001 block height is at least 10 within 200 seconds
alice: parachain 2002 block height is at least 10 within 200 seconds
alice: parachain 2003 block height is at least 10 within 200 seconds

# Check if disputes are initiated and concluded.
# TODO: check if disputes are concluded faster than initiated.
eve: reports parachain_candidate_disputes_total is at least 10 within 15 seconds
eve: reports parachain_candidate_dispute_concluded{validity="valid"} is at least 10 within 15 seconds
eve: reports parachain_candidate_dispute_concluded{validity="invalid"} is 0 within 15 seconds

# Check there is an offence report
alice: system event contains "There is an offence reported" within 60 seconds

# Check lag - approval
alice: reports polkadot_parachain_approval_checking_finality_lag is 0
bob: reports polkadot_parachain_approval_checking_finality_lag is 0
charlie: reports polkadot_parachain_approval_checking_finality_lag is 0
dave: reports polkadot_parachain_approval_checking_finality_lag is 0
ferdie: reports polkadot_parachain_approval_checking_finality_lag is 0
eve: reports polkadot_parachain_approval_checking_finality_lag is 0
one: reports polkadot_parachain_approval_checking_finality_lag is 0
two: reports polkadot_parachain_approval_checking_finality_lag is 0

# Check lag - dispute conclusion
alice: reports polkadot_parachain_disputes_finality_lag is 0
bob: reports polkadot_parachain_disputes_finality_lag is 0
charlie: reports polkadot_parachain_disputes_finality_lag is 0
dave: reports polkadot_parachain_disputes_finality_lag is 0
ferdie: reports polkadot_parachain_disputes_finality_lag is 0
eve: reports polkadot_parachain_disputes_finality_lag is 0
one: reports polkadot_parachain_disputes_finality_lag is 0
two: reports polkadot_parachain_disputes_finality_lag is 0
