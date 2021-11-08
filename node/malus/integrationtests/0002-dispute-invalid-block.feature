Description: Disputes
Network: ./0002-dispute-invalid-block.toml
Creds: config.gcloud

alice: is up
alice: reports node_roles is 4
alice: reports sub_libp2p_is_major_syncing is 0
alice: reports block height is greater than 2 within 15 seconds
alice: reports peers count is at least 2
bob: is up
bob: reports block height is greater than 2
bob: reports peers count is at least 2
charlie: is up
charlie: reports block height is greater than 2
charlie: reports peers count is at least 2
david: is up
#eve: is up
alice: reports parachain_candidate_disputes_total is at least 1 within 225 seconds 
alice: reports parachain_candidate_dispute_votes{validity="invalid"} is at least 1 within 30 seconds
bob: reports parachain_candidate_dispute_votes{validity="invalid"} is 2 within 30 seconds
charlie: reports parachain_candidate_dispute_votes{validity="invalid"} is 3 within 30 seconds
david: reports parachain_candidate_dispute_votes{validity="invalid"} is 4 within 30 seconds
alice: reports parachain_candidate_dispute_concluded{validity="invalid"} is 1 within 30 seconds
