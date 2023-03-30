async function run(nodeName, networkInfo, args) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);

  const proof = await api.rpc.mmr.generateProof([1]);

  const proofVerifications = await Promise.all(
    args.map(async (nodeName) => {
      const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
      const api = await zombie.connect(wsUri, userDefinedTypes);
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
