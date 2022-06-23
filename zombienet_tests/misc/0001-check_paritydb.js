const fs = require('fs');

const { exec } = require("child_process");

// Check if we are running paritydb on a specified node.
async function run(nodeName, networkInfo, jsArgs) {
    console.log(networkInfo);
    let node_path = networkInfo.tmpDir;
    // TODO: We need Zombienet to provide the chain spec name in networkInfo to un-hardcode this path.
    let dir = `${node_path}/${nodeName}/data/chains/rococo_local_testnet/paritydb/full`;

    const { exec } = require("child_process");

    exec(`ls -lahR ${node_path}/${nodeName}`, (error, stdout, stderr) => {
        if (error) {
            console.log(`error: ${error.message}`);
            return;
        }
        if (stderr) {
            console.log(`stderr: ${stderr}`);
            return;
        }
        console.log(`stdout: ${stdout}`);
    });

    // Check if directory exists
    if (!fs.existsSync(dir)) {
        console.log('ParityDB path not found!');
        return -1;
    }

    return 0
}


module.exports = { run }