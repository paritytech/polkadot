Feature: Disputes

  Scenario: Dispute Invalid Block
    Given a test network
    Then alice is up
    And alice reports node_roles is 4
    And alice reports sub_libp2p_is_major_syncing is 0
    Then sleep 60 seconds
    Then alice reports block height is greater than 10
    And alice reports peers count is at least 2
    Then bob is up
    And bob reports block height is greater than 10
    And bob reports peers count is at least 2
    Then charlie is up
    And charlie reports block height is greater than 10
    And charlie reports peers count is at least 2
    Then david is up
    Then eve is up
    And alice reports parachain_candidate_open_disputes is 1
    And bob reports parachain_candidate_open_disputes is 1
    And charlie reports parachain_candidate_open_disputes is 1
    Then alice parachain_candidate_dispute_votes is at least 1
    And bob parachain_candidate_dispute_votes is is at least 2
    And charlie parachain_candidate_dispute_votes is at least 3
    Then alice parachain_candidate_dispute_concluded is "valid"
    And bob parachain_candidate_dispute_concluded is "valid"
    And charlie parachain_candidate_dispute_concluded is "valid"
