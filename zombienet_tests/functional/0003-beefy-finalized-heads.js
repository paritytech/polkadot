async function run(_, networkInfo, nodeNames) {
  const apis = await Promise.all(
    nodeNames.map(async (nodeName) => {
      const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
      return await zombie.connect(wsUri, userDefinedTypes);
    })
  );

  const finalizedHeads = await Promise.all(
    apis.map(async (api) => {
      return api.rpc.beefy.getFinalizedHead();
    })
  );

  const finalizedHeadsHeight = await Promise.all(
    finalizedHeads.map(async (finalizedHead) => {
      return apis[0].rpc.chain.getHeader(finalizedHead).then((header) => header.number);
    })
  );

  // check that all nodes agree on block height up to a tolerance of at most 1
  return finalizedHeadsHeight.every((height) => Math.abs(height - finalizedHeadsHeight[0]) <= 1)
}

module.exports = { run };
