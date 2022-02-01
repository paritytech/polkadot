Description: PVF preparation & execution time
Network: ./small-rococo-net.toml
Creds: config

# Some sanity checks
alice: is up
bob: is up
charlie: is up
dave: is up
eve: is up
ferdie: is up
one: is up
two: is up

# Check authority status and peers.
# MAYBE FEATURE NEEDED: Have a sanity check section before running the actual test, such that 
# we can gate/sync execution on some preconditions that must been met.
alice: reports node_roles is 4
bob: reports node_roles is 4
charlie: reports node_roles is 4
dave: reports node_roles is 4
eve: reports node_roles is 4
ferdie: reports node_roles is 4
one: reports node_roles is 4
two: reports node_roles is 4

# TODO: Collator metrics ?
# collator01-1: reports peers count is at least 1 within 150 seconds

# Ensure parachains are registered.
alice: parachain 2000 is registered within 60 seconds
bob: parachain 2001 is registered within 60 seconds
charlie: parachain 2002 is registered within 60 seconds
dave: parachain 2003 is registered within 60 seconds
ferdie: parachain 2004 is registered within 60 seconds
eve: parachain 2005 is registered within 60 seconds
one: parachain 2006 is registered within 60 seconds
two: parachain 2007 is registered within 60 seconds

# Ensure parachains made progress.
alice: parachain 2000 block height is at least 10 within 300 seconds
alice: parachain 2001 block height is at least 10 within 300 seconds
alice: parachain 2002 block height is at least 10 within 300 seconds
alice: parachain 2003 block height is at least 10 within 300 seconds
alice: parachain 2004 block height is at least 10 within 300 seconds
alice: parachain 2005 block height is at least 10 within 300 seconds
alice: parachain 2006 block height is at least 10 within 300 seconds
alice: parachain 2007 block height is at least 10 within 300 seconds

#alice: reports pvf_preparation_time_bucket{le="10.0"} is at least 1 within 300 seconds
#bob: reports pvf_preparation_time_bucket{le="10.0"} is 1 within 300 seconds
#charlie: reports pvf_preparation_time_bucket{le="10.0"} is 1 within 300 seconds
#dave: reports pvf_preparation_time_bucket{le="10.0"} is 1 within 300 seconds
#ferdie: reports pvf_preparation_time_bucket{le="10.0"} is 1 within 300 seconds
#eve: reports pvf_preparation_time_bucket{le="10.0"} is 1 within 300 seconds
#one: reports pvf_preparation_time_bucket{le="10.0"} is 1 within 300 seconds
#two: reports pvf_preparation_time_bucket{le="10.0"} is 1 within 300 seconds

# Check execution time.
# There are two different timeout conditions: BACKING_EXECUTION_TIMEOUT(2s) and
# APPROVAL_EXECUTION_TIMEOUT(6s). Currently these are not differentiated by metrics
# because the metrics are defined in `polkadot-node-core-pvf` which is a level below
# the relevant subsystems.
# That being said, we will take the simplifying assumption of testing only the 
# 2s timeout.
#alice: reports pvf_execution_time_bucket{le="2.0"} is 1 within 300 seconds
#bob: reports pvf_execution_time_bucket{le="2.0"} is 1 within 300 seconds
#charlie: reports pvf_execution_time_bucket{le="2.0"} is 1 within 300 seconds
#dave: reports pvf_execution_time_bucket{le="2.0"} is 1 within 300 seconds
#ferdie: reports pvf_execution_time_bucket{le="2.0"} is 1 within 300 seconds
#eve: reports pvf_execution_time_bucket{le="2.0"} is 1 within 300 seconds
#one: reports pvf_execution_time_bucket{le="2.0"} is 1 within 300 seconds
#two: reports pvf_execution_time_bucket{le="2.0"} is 1 within 300 seconds

#sleep 10000 seconds
