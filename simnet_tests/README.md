# Simulation tests, or high level integration tests

_The content of this directory is meant to be used by Parity's private CI/CD
infrastructure with private tools. At the moment those tools are still early
stage of development and we don't know if / when they will available for
public use._

## Contents of this directory

`configs` directory contains config files in toml format that describe how to
configure the simulation network that you want to launch.

`tests` directory contains [Cucumber](https://cucumber.io/) files. Those are
Behavior-Driven Development test files that describe tests in plain English.
Under the hood there are assertions that specific metrics should have specific
values.

At the moment we have only one test for parachains: `/parachains.features`
This test uses a JS script that we added to Simnet image and it's launched
by this step in the cucumber file:
`Then launch 'node' with parameters '--unhandled-rejections=strict /usr/local/bin/simnet_scripts test_parachain ./configs/adder.json ws://localhost:11222 100 10'`

`run_test.sh` is an entry point for running all tests in the folder.
Any setup required for tests (but cannot be done in configs) is performed
here. The main script's responsibility is to run [Gurke](https://github.com/paritytech/gurke)
with passed parameters.
In order to use this script locally, you need to install
[Gurke](https://github.com/paritytech/gurke)
Once you have access to a kubernetes cluster (meaning you can do `kubectl get pods`)
you can run this script with no arguments, like `./run_test.sh` and tests should run.
Kubernetes cluster can be local, spawned with
[kind](https://kind.sigs.k8s.io/docs/user/quick-start/#installation)
or an instance living in the
[cloud](https://github.com/paritytech/gurke/blob/main/docs/How-to-setup-access-to-gke-k8s-cluster.md)

### [Here is link to barcamp presentation of Simnet](https://www.crowdcast.io/e/ph49xu01)

### [Here is link to the Simnet repo, hosted on private gitlab](https://gitlab.parity.io/parity/simnet/-/tree/master)
