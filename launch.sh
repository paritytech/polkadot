#        ________  ________  ___       ___  __    ________  ________  ________  _________   
#       |\   __  \|\   __  \|\  \     |\  \|\  \ |\   __  \|\   ___ \|\   __  \|\___   ___\ 
#       \ \  \|\  \ \  \|\  \ \  \    \ \  \/  /|\ \  \|\  \ \  \_|\ \ \  \|\  \|___ \  \_| 
#        \ \   ____\ \  \\\  \ \  \    \ \   ___  \ \   __  \ \  \ \\ \ \  \\\  \   \ \  \  
#         \ \  \___|\ \  \\\  \ \  \____\ \  \\ \  \ \  \ \  \ \  \_\\ \ \  \\\  \   \ \  \ 
#          \ \__\    \ \_______\ \_______\ \__\\ \__\ \__\ \__\ \_______\ \_______\   \ \__\
#           \|__|     \|_______|\|_______|\|__| \|__|\|__|\|__|\|_______|\|_______|    \|__|
#                                                                                           
#                                                                                           
#                                                                                           
#               ________  ________  ___  ___  ________  ________  _______   ________        
#              |\   ____\|\   __  \|\  \|\  \|\   __  \|\   __  \|\  ___ \ |\   ___ \       
#              \ \  \___|\ \  \|\  \ \  \\\  \ \  \|\  \ \  \|\  \ \   __/|\ \  \_|\ \      
#               \ \_____  \ \  \\\  \ \  \\\  \ \   __  \ \   _  _\ \  \_|/_\ \  \ \\ \     
#                \|____|\  \ \  \\\  \ \  \\\  \ \  \ \  \ \  \\  \\ \  \_|\ \ \  \_\\ \    
#                  ____\_\  \ \_____  \ \_______\ \__\ \__\ \__\\ _\\ \_______\ \_______\   
#                 |\_________\|___| \__\|_______|\|__|\|__|\|__|\|__|\|_______|\|_______|   
#                 \|_________|     \|__|                                                    
#             
# Hacky script to spin up relay-chains and setup bridge stuff
#
# # Quick links.
# Make sure to forward those ports though.
#
# Rococo:
# - https://polkadot.js.org/apps/?rpc=ws%3A%2F%2F127.0.0.1%3A9900#/explorer
# - https://polkadot.js.org/apps/?rpc=ws%3A%2F%2F127.0.0.1%3A9901#/explorer
#
# Wococo:
# - https://polkadot.js.org/apps/?rpc=ws%3A%2F%2F127.0.0.1%3A9902#/explorer
# - https://polkadot.js.org/apps/?rpc=ws%3A%2F%2F127.0.0.1%3A9903#/explorer

BIN=./target/release/polkadot

if [[ -z "${RELAY}" ]]; then
    echo "RELAY not set"
    exit 1
fi

# trap catches a signal and executes the specified command, i.e. `wait`.
#
# PID0 is a special case. It represents all the processes that was spawned by the parent process,
# that is, this script.
trap 'kill 0' EXIT

$BIN build-spec --chain=rococo > rococo.json

chains=("rococo" "wococo")
validators=(alice bob)

# Pregenerated node keys and corresponding PeerIDs.
node_keys=("f1c82f30c6df4996ec9135a468d0748726ee105501bf9987cdc541ce6b85528e" "399503fd4b31b3219b0122d2cbcd41406aa61210de3fb6711febccdaef40947a")
node_peerids=("12D3KooWAosxdpRN43ecLx9t66voPDm3SfNX3S1fdCiXq6Zs1fxa" "12D3KooWDsSCa2zo1GrPEDfCAnd5UdXGGdpUePQ3f8fJQ3kzUxGM")

# We need to fund this address, otherwise you won't be able to pay for the relaying.
UNIVERSAL_RELAYER_FUND_SS58="5HQn2URwTz6CPsZzbtPzm93KEza881uQ92rLRhtRzSLoA7ws"

CHAIN_NUM=2
VALIDATOR_NUM=2

BASE_PORT=9700
BASE_RPC_PORT=9800
BASE_WS_PORT=9900

export RUST_LOG=runtime=trace

rm -r logs
mkdir -p logs

for ((chain_idx=0;chain_idx<CHAIN_NUM;chain_idx++)); do
    chain=${chains[$chain_idx]}

    echo "Starting $chain"
    $BIN build-spec --chain=$chain-dev > $chain.json
    
    for ((validator_idx=0;validator_idx<VALIDATOR_NUM;validator_idx++)); do
        id=$chain-validator-$validator_idx
        validator=${validators[$validator_idx]}
    
        echo "  Starting validator $id"
        
        $BIN --tmp \
            --validator \
            --rpc-cors=all --unsafe-rpc-external --unsafe-ws-external \
            --no-hardware-benchmarks \
            --$validator \
            --force-authoring \
            --name $id \
            --chain=$chain.json \
            --node-key=${node_keys[$validator_idx]} \
            --bootnodes=/ip4/127.0.0.1/tcp/$((BASE_PORT + chain_idx * CHAIN_NUM + 0))/p2p/${node_peerids[0]} \
            --port $((BASE_PORT + chain_idx * CHAIN_NUM + validator_idx)) \
            --rpc-port $((BASE_RPC_PORT + chain_idx * CHAIN_NUM + validator_idx)) \
            --ws-port $((BASE_WS_PORT + chain_idx * CHAIN_NUM + validator_idx)) \
            > logs/$id.log 2>&1 &
        echo "Launched"
    done
done

# Let the nodes start up. Without this it will still work, but the retry time is 10s.
echo "All relay-chain nodes are launched. Waiting for them to come online..."
sleep 5

# Initialize bridges

$RELAY init-bridge rococo-to-wococo \
        --source-host 127.0.0.1 \
        --source-port 9900 \
        --target-host 127.0.0.1 \
        --target-port 9902 \
        --target-signer "//Alice"
        
$RELAY init-bridge wococo-to-rococo \
        --source-host 127.0.0.1 \
        --source-port 9900 \
        --target-host 127.0.0.1 \
        --target-port 9902 \
        --target-signer "//Alice"

$RELAY relay-headers rococo-to-wococo \
        --source-host 127.0.0.1 \
        --source-port 9900 \
        --target-host 127.0.0.1 \
        --target-port 9902 \
        --target-signer "//Alice" \
        > logs/message-messages.log 2>&1 &
        
$RELAY relay-messages rococo-to-wococo \
        --source-host 127.0.0.1 \
        --source-port 9900 \
        --target-host 127.0.0.1 \
        --target-port 9902 \
        --source-signer "//Alice" \
        --target-signer "//Alice" \
        --relayer-mode=altruistic \
        --lane 00000000 > logs/message-relayer.log 2>&1 &

wait
echo "All children is dead. Exiting."
