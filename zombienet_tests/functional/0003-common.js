async function getApis(networkInfo, nodeNames) {
  return await Promise.all(
    nodeNames.map(async (nodeName) => {
      const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
      return await zombie.connect(wsUri, userDefinedTypes);
    })
  )
}

module.exports = { getApis };
