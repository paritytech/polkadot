Description: Check that paritydb works without affecting finality lag and block production.
Network: ./0001-paritydb.toml
Creds: config

validator-0: is up
validator-1: is up
validator-2: is up
validator-3: is up
validator-4: is up
validator-5: is up
validator-6: is up
validator-7: is up
validator-8: is up
validator-9: is up

# Check if we are using ParityDB.
validator: log line contains "Database: ParityDb"

# Check authority status and peers.
validator-0: reports node_roles is 4
validator-1: reports node_roles is 4
validator-2: reports node_roles is 4
validator-3: reports node_roles is 4
validator-4: reports node_roles is 4
validator-5: reports node_roles is 4
validator-6: reports node_roles is 4
validator-7: reports node_roles is 4
validator-8: reports node_roles is 4
validator-9: reports node_roles is 4

# Ensure parachains are registered.
validator-0: parachain 2000 is registered
validator-0: parachain 2001 is registered
validator-0: parachain 2002 is registered
validator-0: parachain 2003 is registered
validator-0: parachain 2004 is registered
validator-0: parachain 2005 is registered
validator-0: parachain 2006 is registered
validator-0: parachain 2007 is registered
validator-0: parachain 2008 is registered
validator-0: parachain 2009 is registered

# Check if network is fully connected.
validator-0: reports peers count is at least 19 within 20 seconds
validator-1: reports peers count is at least 19 within 20 seconds
validator-2: reports peers count is at least 19 within 20 seconds
validator-3: reports peers count is at least 19 within 20 seconds
validator-4: reports peers count is at least 19 within 20 seconds
validator-5: reports peers count is at least 19 within 20 seconds
validator-6: reports peers count is at least 19 within 20 seconds
validator-7: reports peers count is at least 19 within 20 seconds
validator-8: reports peers count is at least 19 within 20 seconds
validator-9: reports peers count is at least 19 within 20 seconds

# Ensure parachains made some progress.
validator-0: parachain 2000 block height is at least 3 within 30 seconds
validator-0: parachain 2001 block height is at least 3 within 30 seconds
validator-0: parachain 2002 block height is at least 3 within 30 seconds
validator-0: parachain 2003 block height is at least 3 within 30 seconds
validator-0: parachain 2004 block height is at least 3 within 30 seconds
validator-0: parachain 2005 block height is at least 3 within 30 seconds
validator-0: parachain 2006 block height is at least 3 within 30 seconds
validator-0: parachain 2007 block height is at least 3 within 30 seconds
validator-0: parachain 2008 block height is at least 3 within 30 seconds
validator-0: parachain 2009 block height is at least 3 within 30 seconds

# Check lag - approval
validator-0: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-1: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-2: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-3: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-4: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-5: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-6: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-7: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-8: reports polkadot_parachain_approval_checking_finality_lag is 0
validator-9: reports polkadot_parachain_approval_checking_finality_lag is 0

# Check lag - dispute conclusion
validator-0: reports parachain_candidate_disputes_total is 0
validator-1: reports parachain_candidate_disputes_total is 0
validator-2: reports parachain_candidate_disputes_total is 0
validator-3: reports parachain_candidate_disputes_total is 0
validator-4: reports parachain_candidate_disputes_total is 0
validator-5: reports parachain_candidate_disputes_total is 0
validator-6: reports parachain_candidate_disputes_total is 0
validator-7: reports parachain_candidate_disputes_total is 0
validator-8: reports parachain_candidate_disputes_total is 0
validator-9: reports parachain_candidate_disputes_total is 0

