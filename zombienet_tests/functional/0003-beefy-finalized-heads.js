const common = require('./0003-common.js');

async function run(_, networkInfo, nodeNames) {
  const apis = await common.getApis(networkInfo, nodeNames);

  const finalizedHeads = await Promise.all(
    Object.entries(apis).map(async ([nodeName, api]) => {
      const finalizedHead = await api.rpc.beefy.getFinalizedHead();
      return { nodeName, finalizedHead, finalizedHeight: await api.rpc.chain.getHeader(finalizedHead).then((header) => header.number) };
    })
  );

  // select the node with the highest finalized height
  const highestFinalizedHeight = finalizedHeads.reduce(
    (acc, { nodeName, finalizedHeight }) =>
      finalizedHeight >= acc.finalizedHeight
        ? { nodeName, finalizedHeight }
        : acc,
    { nodeName: 'validator', finalizedHeight: 0 }
  );

  // get all block hashes up until the highest finalized height
  const blockHashes = [];
  for (let blockNumber = 0; blockNumber <= highestFinalizedHeight.finalizedHeight; blockNumber++) {
    const blockHash = await apis[highestFinalizedHeight.nodeName].rpc.chain.getBlockHash(blockNumber);
    blockHashes.push(blockHash);
  }

  // verify that height(finalized_head) is at least as high as the substrate_beefy_best_block test already verified
  return finalizedHeads.every(({ finalizedHead, finalizedHeight }) =>
    finalizedHeight >= 21 && finalizedHead.toHex() === blockHashes[finalizedHeight].toHex()
  )
}

module.exports = { run };
