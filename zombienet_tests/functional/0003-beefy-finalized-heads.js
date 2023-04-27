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

  // verify that height(finalized_head) is at least as high as the substrate_beefy_best_block test already verified
  return finalizedHeadsHeight.every((height) => height >= 21)
}

module.exports = { run };
