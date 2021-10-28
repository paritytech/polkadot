Description: Disputes
Network: ./0001-dispute-valid-block.toml
Creds: config.gcloud


alice: is up
bob: is up
charlie: is up
#david is up
alice: reports node_roles is 4
alice: reports sub_libp2p_is_major_syncing is 0
#sleep 15 seconds
alice: reports block height is at least 2 within 15 seconds
alice: reports peers count is at least 2
bob: reports block height is at least 2
bob: reports peers count is at least 2
charlie: reports block height is at least 2
charlie: reports peers count is at least 2
#sleep 121 seconds
alice: reports parachain_candidate_disputes_total is at least 1 within 125 seconds
bob: reports parachain_candidate_disputes_total is at least 1
charlie: reports parachain_candidate_disputes_total is at least 1
alice: reports parachain_candidate_dispute_votes{validity="valid"} is at least 1
bob: reports parachain_candidate_dispute_votes{validity="valid"} is at least 2
charlie: reports parachain_candidate_dispute_votes{validity="valid"} is at least 2
alice: reports parachain_candidate_dispute_concluded{validity="valid"} is at least 1
alice: reports parachain_candidate_dispute_concluded{validity="invalid"} is 0
bob: reports parachain_candidate_dispute_concluded{validity="valid"} is at least 1
charlie: reports parachain_candidate_dispute_concluded{validity="valid"} is at least 1
