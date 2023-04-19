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

  // check that the finalized heads are the same
  return finalizedHeads.every((finalizedHead) => finalizedHead.eq(finalizedHeads[0]))
}

module.exports = { run };
