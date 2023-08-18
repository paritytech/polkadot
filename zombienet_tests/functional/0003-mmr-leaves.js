async function run(nodeName, networkInfo, args) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);

  const mmrLeaves = await api.query.mmr.numberOfLeaves();
  return mmrLeaves.toNumber() >= args[0]
}

module.exports = { run };
