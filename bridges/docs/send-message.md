# How to send messages

The Substrate-to-Substrate relay comes with a command line interface (CLI) which is implemented
by the `substrate-relay` binary.

```
Substrate-to-Substrate relay

USAGE:
    substrate-relay <SUBCOMMAND>

FLAGS:
    -h, --help       
            Prints help information

    -V, --version    
            Prints version information


SUBCOMMANDS:
    help              Prints this message or the help of the given subcommand(s)
    init-bridge       Initialize on-chain bridge pallet with current header data
    relay-headers     Start headers relay between two chains
    relay-messages    Start messages relay between two chains
    send-message      Send custom message over the bridge
```
The relay related commands `relay-headers` and `relay-messages` are basically continously running a
sync loop between the `Millau` and `Rialto` chains. The `init-bridge` command submitts initialization
transactions. An initialization transaction brings an initial header and authorities set from a source
chain to a target chain. The header synchronization then starts from that header.

For sending custom messages over an avialable bridge, the `send-message` command is used.

```
Send custom message over the bridge.

Allows interacting with the bridge by sending messages over `MessageLane` component. The message is being sent to the
source chain, delivered to the target chain and dispatched there.

USAGE:
    substrate-relay send-message <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    help                Prints this message or the help of the given subcommand(s)
    millau-to-rialto    Submit message to given Millau -> Rialto lane
    rialto-to-millau    Submit message to given Rialto -> Millau lane

```
Messages are send from a source chain to a target chain using a so called `message lane`. Message lanes handle
both, message transport and message dispatch. There is one command for submitting a message to each of the two
available bridges, namely `millau-to-rialto` and `rialto-to-millau`.

Submitting a message requires a number of arguments to be provided. Those arguments are essentially the same
for both submit message commands, hence only the output for `millau-to-rialto` is shown below.

```
Submit message to given Millau -> Rialto lane

USAGE:
    substrate-relay send-message millau-to-rialto [OPTIONS] --lane <lane> --millau-host <millau-host> --millau-port <millau-port> --millau-signer <millau-signer> --origin <origin> --rialto-signer <rialto-signer> <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --fee <fee>
            Delivery and dispatch fee. If not passed, determined automatically

        --lane <lane>                                        Hex-encoded lane id
        --millau-host <millau-host>                          Connect to Millau node at given host
        --millau-port <millau-port>                          Connect to Millau node websocket server at given port
        --millau-signer <millau-signer>
            The SURI of secret key to use when transactions are submitted to the Millau node

        --millau-signer-password <millau-signer-password>
            The password for the SURI of secret key to use when transactions are submitted to the Millau node

        --origin <origin>
            The origin to use when dispatching the message on the target chain [possible values: Target, Source]

        --rialto-signer <rialto-signer>
            The SURI of secret key to use when transactions are submitted to the Rialto node

        --rialto-signer-password <rialto-signer-password>
            The password for the SURI of secret key to use when transactions are submitted to the Rialto node


SUBCOMMANDS:
    help        Prints this message or the help of the given subcommand(s)
    remark      Make an on-chain remark (comment)
    transfer    Transfer the specified `amount` of native tokens to a particular `recipient`

```
As can be seen from the output, there are two types of messages available: `remark` and `transfer`.
A remark is some opaque message which will be placed on-chain. For basic testing, a remark is
the easiest to go with.

Usage of the arguments is best explained with an example. Below you can see, how a remark 
would look like:

```
substrate-relay send-message millau-to-rialto \
		--millau-host=127.0.0.1 \
		--millau-port=10946 \
		--millau-signer=//Dave \
		--rialto-signer=//Dave \
		--lane=00000000 \
		--origin Target \
		remark
```
Messages are basically regular transactions. That means, they have to be signed. In order
to send a message, you have to control an account private key on both, the source and
the target chain. Those accounts are specified using the `--millau-signer` and `--rialto-signer`
arguments in the example above. 

Message delivery and dispatch requires a fee to be paid. In the example above, we have not 
specified the `--fee` argument. Hence, the fee will be estimated automatically. Note that
in order to pay the fee, the message sender account has to have sufficient funds available.

The `--origin` argument allows to denote under which authority the message will be dispatched
on the target chain. Accepted values are `Target` and `Source`. 

Although not strictly necessary, it is recommended, to use one of the well-known development
accounts (`Alice`, `Bob`, `Charlie`, `Dave`, `Eve`) for message sending. Those accounts are
endowed with funds for fee payment. In addtion, the development `Seed URI` syntax
(like `//Dave`) for the signer can be used, which will remove the need for a password.
