Feature: ParaTesting

  Scenario: spawn parachains network and check parachains
    Given a test network
    Then sleep 200 seconds
    Then launch 'node' with parameters '/usr/local/bin/simnet_scripts/dist/index.js test_parachain /usr/local/bin/simnet_scripts/type_defs/adder.json ws://localhost:11222 100 10'
