# nginx-proxy

This is a nginx reverse proxy configuration with Let's encrypt companion.
Main purpose is to be able to use `https://polkadot.js.org/apps` to connect to
a running network.

## How to?

In current directory:
```bash
docker-compose up -d
```

Then start `rialto` network with the same command (one folder up). `nginx` should
pick up new containers being created and automatically create a proxy setup for `Charlie`.
