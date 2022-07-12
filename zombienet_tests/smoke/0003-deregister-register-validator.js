const assert = require("assert");

function nameCase(string) {
	return string.charAt(0).toUpperCase() + string.slice(1);
}

async function run(nodeName, networkInfo, jsArgs) {
    const {wsUri, userDefinedTypes} = networkInfo.nodesByName[nodeName];
    const api = await zombie.connect(wsUri, userDefinedTypes);
    const action = jsArgs[0] === "register" ? "registerValidators" : "deregisterValidators"
    const validatorName = jsArgs[1]; // used as seed

    await zombie.util.cryptoWaitReady();

    // account to submit tx
    const keyring = new zombie.Keyring({ type: "sr25519" });
    const alice = keyring.addFromUri("//Alice");
    const validatorStash = keyring.createFromUri(`//${nameCase(validatorName)}//stash`);

    await new Promise(async (resolve, reject) => {
        const unsub = await api.tx.sudo
          .sudo(api.tx.validatorManager[action]([validatorStash.address]))
          .signAndSend(alice, (result) => {
            console.log(`Current status is ${result.status}`);
            if (result.status.isInBlock) {
              console.log(
                `Transaction included at blockHash ${result.status.asInBlock}`
              );
            } else if (result.status.isFinalized) {
              console.log(
                `Transaction finalized at blockHash ${result.status.asFinalized}`
              );
              unsub();
              return resolve();
            } else if (result.isError) {
              console.log(`Transaction Error`);
              unsub();
              return reject();
            }
          });
    });

    return 0;
}

module.exports = { run }
