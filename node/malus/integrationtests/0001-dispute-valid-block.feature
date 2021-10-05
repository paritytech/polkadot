Feature: Disputes

  Scenario: Dispute Valid Block
    Given a test network
    Then alice is up
    And alice reports node_roles is 4
    And alice reports sub_libp2p_is_major_syncing is 0
    Then sleep 15 seconds
    Then alice reports block height is at least 2
    And alice reports peers count is at least 2
    Then bob is up
    And bob reports block height is at least 2
    And bob reports peers count is at least 2
    Then charlie is up
    And charlie reports block height is at least 2
    And charlie reports peers count is at least 2
    Then david is up
    Then sleep 121 seconds
    And alice reports polkadot_parachain_candidate_disputes_total is at least 1
    And bob reports polkadot_parachain_candidate_disputes_total is at least 1
    And charlie reports polkadot_parachain_candidate_disputes_total is at least 1
    Then alice polkadot_parachain_candidate_dispute_votes{validity="valid"} is at least 1
    And bob polkadot_parachain_candidate_dispute_votes{validity="valid"} is at least 2
    And charlie polkadot_parachain_candidate_dispute_votes{validity="valid"} is at least 2
    Then alice polkadot_parachain_candidate_dispute_concluded{validity="valid"} is at least 1
    Then alice polkadot_parachain_candidate_dispute_concluded{validity="invalid"} is 0
    Then bob polkadot_parachain_candidate_dispute_concluded{validity="valid"} is at least 1
    And charlie polkadot_parachain_candidate_dispute_concluded{validity="valid"} is at least 1
