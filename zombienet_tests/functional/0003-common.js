async function getApis(networkInfo, nodeNames) {
  const connectionPromises = nodeNames.map(async (nodeName) => {
    const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
    const connection = await zombie.connect(wsUri, userDefinedTypes);
    return { nodeName, connection };
  });

  const connections = await Promise.all(connectionPromises);

  return connections.reduce((map, { nodeName, connection }) => {
    map[nodeName] = connection;
    return map;
  }, {});
}

module.exports = { getApis };
