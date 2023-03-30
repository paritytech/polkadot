async function run(nodeName, networkInfo, _) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);

  const proof = await api.rpc.mmr.generateProof([1]);

  if (await api.rpc.mmr.verifyProof(proof)) {
    return 1;
  } else {
    return 0;
  }
}

module.exports = { run };
