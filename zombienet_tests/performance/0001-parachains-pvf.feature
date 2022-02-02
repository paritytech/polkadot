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

# Check authority status.
alice: reports node_roles is 4
bob: reports node_roles is 4
charlie: reports node_roles is 4
dave: reports node_roles is 4
eve: reports node_roles is 4
ferdie: reports node_roles is 4
one: reports node_roles is 4
two: reports node_roles is 4

# TODO: Collator metric/logs checks ?

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

# Check preparation time is under 10s.
# Check all buckets <= 10.
alice: reports histogram polkadot_pvf_preparation_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2", "3", "10"] within 300 seconds
bob: reports histogram polkadot_pvf_preparation_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2", "3", "10"] within 300 seconds
charlie: reports histogram polkadot_pvf_preparation_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2", "3", "10"] within 300 seconds
dave: reports histogram polkadot_pvf_preparation_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2", "3", "10"] within 300 seconds
ferdie: reports histogram polkadot_pvf_preparation_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2", "3", "10"] within 300 seconds
eve: reports histogram polkadot_pvf_preparation_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2", "3", "10"] within 300 seconds
one: reports histogram polkadot_pvf_preparation_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2", "3", "10"] within 300 seconds
two: reports histogram polkadot_pvf_preparation_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2", "3", "10"] within 300 seconds

# Check all buckets >= 20.             
alice: reports histogram polkadot_pvf_preparation_time has 0 samples in buckets ["20", "30", "60", "+Inf"] within 300 seconds
bob: reports histogram polkadot_pvf_preparation_time has 0 samples in buckets ["20", "30", "60", "+Inf"] within 300 seconds
charlie: reports histogram polkadot_pvf_preparation_time has 0 samples in buckets ["20", "30", "60", "+Inf"] within 300 seconds
dave: reports histogram polkadot_pvf_preparation_time has 0 samples in buckets ["20", "30", "60", "+Inf"] within 300 seconds
ferdie: reports histogram polkadot_pvf_preparation_time has 0 samples in buckets ["20", "30", "60", "+Inf"] within 300 seconds
eve: reports histogram polkadot_pvf_preparation_time has 0 samples in buckets ["20", "30", "60", "+Inf"] within 300 seconds
one: reports histogram polkadot_pvf_preparation_time has 0 samples in buckets ["20", "30", "60", "+Inf"] within 300 seconds
two: reports histogram polkadot_pvf_preparation_time has 0 samples in buckets ["20", "30", "60", "+Inf"] within 300 seconds

# Check execution time.
# There are two different timeout conditions: BACKING_EXECUTION_TIMEOUT(2s) and
# APPROVAL_EXECUTION_TIMEOUT(6s). Currently these are not differentiated by metrics
# because the metrics are defined in `polkadot-node-core-pvf` which is a level below
# the relevant subsystems.
# That being said, we will take the simplifying assumption of testing only the 
# 2s timeout. 
# We do this check by ensuring all executions fall into bucket le="2" or lower.
# First, check if we have at least 1 sample, but we should have many more.
alice: reports histogram polkadot_pvf_execution_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2"] within 300 seconds
bob: reports histogram polkadot_pvf_execution_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2"] within 300 seconds
charlie: reports histogram polkadot_pvf_execution_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2"] within 300 seconds
dave: reports histogram polkadot_pvf_execution_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2"] within 300 seconds
ferdie: reports histogram polkadot_pvf_execution_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2"] within 300 seconds
eve: reports histogram polkadot_pvf_execution_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2"] within 300 seconds
one: reports histogram polkadot_pvf_execution_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2"] within 300 seconds
two: reports histogram polkadot_pvf_execution_time has at least 1 samples in buckets ["0.1", "0.5", "1", "2"] within 300 seconds

# Check if we have no samples > 2s.
alice: reports histogram polkadot_pvf_execution_time has 0 samples in buckets ["3", "4", "5", "6", "+Inf"] within 300 seconds
bob: reports histogram polkadot_pvf_execution_time has 0 samples in buckets ["3", "4", "5", "6", "+Inf"] within 300 seconds
charlie: reports histogram polkadot_pvf_execution_time has 0 samples in buckets ["3", "4", "5", "6", "+Inf"] within 300 seconds
dave: reports histogram polkadot_pvf_execution_time has 0 samples in buckets ["3", "4", "5", "6", "+Inf"] within 300 seconds
ferdie: reports histogram polkadot_pvf_execution_time has 0 samples in buckets ["3", "4", "5", "6", "+Inf"] within 300 seconds
eve: reports histogram polkadot_pvf_execution_time has 0 samples in buckets ["3", "4", "5", "6", "+Inf"] within 300 seconds
one: reports histogram polkadot_pvf_execution_time has 0 samples in buckets ["3", "4", "5", "6", "+Inf"] within 300 seconds
two: reports histogram polkadot_pvf_execution_time has 0 samples in buckets ["3", "4", "5", "6", "+Inf"] within 300 seconds
