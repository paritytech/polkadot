const assert = require("assert");

function nameCase(string) {
	return string.charAt(0).toUpperCase() + string.slice(1);
}

async function run(nodeName, networkInfo, jsArgs) {
    const {wsUri, userDefinedTypes} = networkInfo.nodesByName[nodeName];
    const api = await zombie.connect(wsUri, userDefinedTypes);
    // Import the test keyring (already has dev keys for Alice, Bob, Charlie, Eve & Ferdie)
    const keyring = new zombie.Keyring({ type: "sr25519" });
    const sender = keyring.addFromUri(jsArgs[0]);
    const recipient = keyring.addFromUri(jsArgs[1]);

    const amount = BigInt(jsArgs[2]);

    await zombie.util.cryptoWaitReady();

    // Get sender and recipients initial balance
    const { recipient_nonce, data: recipient_balance } = await api.query.system.account(recipient.address);
    const recipient_initial_balance = BigInt(recipient_balance.free)
    const recipient_final_balance = recipient_initial_balance + amount;
    console.log(`Recipient's current balance is ${recipient_initial_balance}. Should end up as ${recipient_final_balance}`);
    const { sender_nonce, data: sender_balance } = await api.query.system.account(sender.address);
    const sender_initial_balance = BigInt(sender_balance.free)
    // Need to estimate the fee to correctly calculate the sender's final balance
    const fee = BigInt((await api.tx.balances.transfer(recipient.address, amount).paymentInfo(sender)).partialFee);
    const sender_final_balance = sender_initial_balance - amount - fee;
    console.log(`Sender's current balance is ${sender_initial_balance}. Should end up as ${sender_final_balance}`);

    // Send a transfer from Alice to Bob
    await new Promise(async (resolve, reject) => {
        const unsub = await api.tx.balances.transfer(recipient.address, amount)
          .signAndSend(sender , (result) => {
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

    await new Promise(async (resolve, reject) => {
        const { recipient_nonce, data: recipient_balance } = await api.query.system.account(recipient.address);
        const { sender_nonce, data: sender_balance } = await api.query.system.account(sender.address);
        assert (recipient_balance.free == recipient_final_balance, "Recipient does not have the correct amount after the transfer: " + recipient_balance.free + " != " + recipient_final_balance);
        assert (sender_balance.free == sender_final_balance, "Sender does not have the correct amount after the transfer: " + sender_balance.free + " != " + sender_final_balance);
        return resolve();
    });

    return 0;
}

module.exports = { run }
