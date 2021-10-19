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

These test suits are run by Polkadot CI in different pipelines, for every commit in PR, for commit into master etc.
It's the responsibility of the test's developer to provide the correct tag for their test, in order to let CI know, when
this test case should be run. For the baseline the existing tags from the existing tests may be used. If these tags are not
sufficient, the new tag may be created. But CI team should be aware of this tag and condition, when this test case should be run.

In order to run a test case locally, you need to install
[Gurke](https://github.com/paritytech/gurke)
Once you have access to a kubernetes cluster (meaning you can do `kubectl get pods`)
you can use Gurke in order to deploy a chain and run the test (see gurke's manual for the commands).
Kubernetes cluster can be local, spawned with
[kind](https://kind.sigs.k8s.io/docs/user/quick-start/#installation)
or an instance living in the
[cloud](https://github.com/paritytech/gurke/blob/main/docs/How-to-setup-access-to-gke-k8s-cluster.md)

## How to add new test cases
New test case may be added either into the existing test suit or with creation of the new test suit.
In any case it's better to create the test and run it locally first, using Gurke (see above).
- In order to add the test case into the existing test suit, the new element (test case) should be added into [[test-cases]] array in test_suit_description.toml of this test suit. The example:

```
# The existing test case
[[test-cases]]
tags = ["all", "smoketest"]
chain-config = "configs/default_local_testnet.toml"
scenarios = ["tests/001-smoketest.feature"]

# The new test case
[[test-cases]]
tags = ["all", "load"]
allowed-to-fail = true
chain-config = "configs/default_local_testnet.toml"
scenarios = ["tests/002-loadtest.feature"]
```
(See note about test case's tags above).

- In order to create a new test suit for the test case, new folder with test suit description file (test_suit_description.toml) should be created. The exact name is mandatory, CI traverses all subfolfders of simnet_tests directory and looks for this file, in order to build the list of test suits. In this description file the general information about the test suit and array of the test cases should be specified. The example of test_suit_description.toml file with some verbose comments:
```
name = "Name of the test suit"
description = "General information about the suit"
# It is the path to the setup script, that may be needed for the test suit.
# You can perform in this script the actions, that are required for the chain deployment, but have to be performed before
# spawing the chain. Like pre-generation of seeds. This script is run by CI before spawing the chain in the cluster
setup-script="setup_script.sh"
# If the config of the test case requires some custom Docker images, the names for these images should be listed in this section
# CI has to provide all these images in the format image_name = some_value in order to run test cases from this suit
# Obviosuly CI should be aware, if the new custom image is added.
required-images = [
	"SYNTHIMAGE",
	"COLIMAGE",
	"SCRIPTSIMAGE",
	"PARACHAINSIMAGE"
]

# Array of the test cases
# Every elements should be started with [[test-cases]]
[[test-cases]]
# See tags information above
tags = ["all", "smoketest"]
# The config, that will be used in order to spawn the chain
chain-config = "configs/simple_rococo_testnet.toml"
# The array of the scenarios, that will be run on the deployed chain
scenarios = ["tests/parachains.feature"]
```

### [Here is link to barcamp presentation of Simnet](https://www.crowdcast.io/e/ph49xu01)

### [Here is link to the Simnet repo, hosted on private gitlab](https://gitlab.parity.io/parity/simnet/-/tree/master)
