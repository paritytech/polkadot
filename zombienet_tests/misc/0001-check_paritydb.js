const fs = require('fs');

const { exec } = require("child_process");

// Check if we are running paritydb on a specified node.
async function run(nodeName, networkInfo, jsArgs) {
    let node_path = networkInfo.tmpDir;
    // TODO: We need Zombienet to provide the chain spec name in networkInfo to un-hardcode this path.
    let dir = `${node_path}/${nodeName}/data/chains/rococo_local_testnet/paritydb/full`;

    // Check if directory exists
    if (!fs.existsSync(dir)) {
        console.log('ParityDB path not found!');
        return -1;
    }

    return 0
}


module.exports = { run }