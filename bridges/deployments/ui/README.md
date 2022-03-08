# bridges-ui

This is a Bridges UI docker configuration file. The source of the Bridges UI code
can be found in [the repository](https://github.com/paritytech/parity-bridges-ui).
The CI should create and publish a docker image that is used by this configuration
file, so that the code is always using the latest version.
The UI is configured to point to local Rialto and Millau nodes to retrieve the require
data.

This image can be used together with `nginx-proxy` to expose the UI externally. See
`VIRTUAL_*` and `LETSENCRYPT_*` environment variables.

After start the UI is available at `http://localhost:8080`

## How to?

In current directory:
```bash
docker-compose up -d
```

Then start `rialto` & `millau` networks with the same command (one folder up) or
run the full setup by using `../run.sh` script.
