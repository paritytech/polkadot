const common = require('./0003-common.js');

async function run(nodeName, networkInfo, nodeNames) {
  const apis = await common.getApis(networkInfo, nodeNames);

  const proof = await apis[nodeName].rpc.mmr.generateProof([1, 9, 20]);

  const root = await apis[nodeName].rpc.mmr.root()

  const proofVerifications = await Promise.all(
    Object.values(apis).map(async (api) => {
      return api.rpc.mmr.verifyProof(proof);
    })
  );

  const proofVerificationsStateless = await Promise.all(
    Object.values(apis).map(async (api) => {
      return api.rpc.mmr.verifyProofStateless(root, proof);
    })
  );

  // check that all nodes accepted the proof
  return proofVerifications.every((proofVerification) => proofVerification) && proofVerificationsStateless.every((proofVerification) => proofVerification)
}

module.exports = { run };
