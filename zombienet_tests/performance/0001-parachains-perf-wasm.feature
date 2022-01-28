Description: PVF preparation performance test. [WIP]
Network: ./small-rococo-net.toml
Creds: config

# Some sanity checks
# FEATURE NEEDED: permit group checks, such that we can check if all validators of a group,
# or even all of them are up
alice: is up
bob: is up

# Check authority count and peers.
# MAYBE FEATURE NEEDED: Have a sanity check section before running the actual test, such that 
# we can gate/sync execution on some preconditions that must been met.
alice: reports node_roles is 4 within 60 seconds
# BUG? Can't match on collator metrics.
collator01-1: reports peers count is at least 1 within 150 seconds

# Ensure parachain has made progress.
alice: parachain 100 is registered within 120 seconds
alice: parachain 100 block height is at least 10 within 350 seconds

# Buckets are cummulative, higher level buckets also count all lower ones, so we
# need to inspect two buckets. 
# FEATURE: handling these with 1 line of code
alice: reports pvf_preparation_time_bucket{le="1"} is 1
alice: reports pvf_preparation_time_bucket{le="10"} is 1
#bob: reports pvf_preparation_time_bucket{le="1"} is 1
#bob: reports pvf_preparation_time_bucket{le="10"} is 1
#charlie: reports pvf_preparation_time_bucket{le="1"} is 1
#charlie: reports pvf_preparation_time_bucket{le="10"} is 1
#dave: reports pvf_preparation_time_bucket{le="1"} is 1
#dave: reports pvf_preparation_time_bucket{le="10"} is 1


sleep 10000 seconds