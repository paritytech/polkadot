
### Local Two-node Testnet

If you want to see the multi-node consensus algorithm in action locally, then
you can create a local testnet. You'll need two terminals open. In one, run:

```bash
polkadot --chain=local --validator --key Alice -d /tmp/alice
```

and in the other, run:

```bash
polkadot --chain=local --validator --key Bob -d /tmp/bob --port 30334 --bootnodes '/ip4/127.0.0.1/tcp/30333/p2p/ALICE_BOOTNODE_ID_HERE'
```

Ensure you replace `ALICE_BOOTNODE_ID_HERE` with the node ID from the output of
the first terminal.
