const common = require('./0003-common.js');

async function run(_, networkInfo, nodeNames) {
  const apis = await common.getApis(networkInfo, nodeNames);

  const finalizedHeads = await Promise.all(
    Object.entries(apis).map(async ([nodeName, api]) => {
      return { nodeName, finalizedHead: await api.rpc.beefy.getFinalizedHead() };
    })
  );

  const finalizedHeadsHeight = await Promise.all(
    finalizedHeads.map(async ({ nodeName, finalizedHead }) => {
      return apis[nodeName].rpc.chain.getHeader(finalizedHead).then((header) => header.number);
    })
  );

  // check that all nodes agree on block height up to a tolerance of at most 1
  return finalizedHeadsHeight.slice(1).every((height) => Math.abs(height - finalizedHeadsHeight[0]) <= 1)
}

module.exports = { run };
