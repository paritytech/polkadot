async function run(_, networkInfo, nodeNames) {
  const apis = await Promise.all(
    nodeNames.map(async (nodeName) => {
      const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
      return await zombie.connect(wsUri, userDefinedTypes);
    })
  );

  // generate proof on arbitrary node
  const proof = await apis[Math.floor(Math.random(0, apis.length - 1))].rpc.mmr.generateProof([1]);

  const proofVerifications = await Promise.all(
    apis.map(async (api) => {
      return api.rpc.mmr.verifyProof(proof);
    })
  );

  // check that all nodes accepted the proof
  if (proofVerifications.every((proofVerification) => proofVerification)) {
    return 1;
  } else {
    return 0;
  }
}

module.exports = { run };
