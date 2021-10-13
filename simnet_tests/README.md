# Simulation tests, or end-to-end tests

_The content of this directory is meant to be used by Parity's private CI/CD
infrastructure with private tools. At the moment those tools are still early
stage of development and we don't know if / when they will available for
public use._

## Contents of this directory

This directory contains different test suits, everyone one of them contains the set of test cases.
Every test suits is defined by its definition file test_suit_description.toml. More information about
structure of test suits and test cases may be found in [SimNet repository](https://gitlab.parity.io/parity/simnet/-/tree/master/ci_helper).

Every test case deploys a test network, using toml config file, and runs the test,
using a test scenario, written in [Cucumber](https://cucumber.io/).

In order to a test case locally, you need to install
[Gurke](https://github.com/paritytech/gurke)
Once you have access to a kubernetes cluster (meaning you can do `kubectl get pods`)
you can use Gurke in order to deploy a chain and run the test (see gurke's manual for the commands).
Kubernetes cluster can be local, spawned with
[kind](https://kind.sigs.k8s.io/docs/user/quick-start/#installation)
or an instance living in the
[cloud](https://github.com/paritytech/gurke/blob/main/docs/How-to-setup-access-to-gke-k8s-cluster.md)

### [Here is link to barcamp presentation of Simnet](https://www.crowdcast.io/e/ph49xu01)

### [Here is link to the Simnet repo, hosted on private gitlab](https://gitlab.parity.io/parity/simnet/-/tree/master)
