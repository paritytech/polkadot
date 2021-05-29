import {  readFileSync } from 'fs';
import { ApiPromise } from "@polkadot/api";
import { ValidatorIndex, CompactAssignments, ElectionScore, EraIndex, ElectionSize } from "@polkadot/types/interfaces/staking"
import { Keyring } from "@polkadot/keyring"
import { join } from "path"

const keyring = new Keyring({ type: 'sr25519', ss58Format: 0 });


async function main() {
	const api = await ApiPromise.create()

	let solutionPath = join(".", process.argv[2]);
	console.log(`++ reading solution binary from path ${solutionPath}`)

	let buffer = readFileSync(solutionPath)
	let bytes = new Uint8Array(buffer)

	// @ts-ignore
	let [winners_raw, compact_raw, score_raw, era_raw, size_raw]: any[] = api.createType('(Vec<ValidatorIndex>, CompactAssignments, ElectionScore, EraIndex, ElectionSize)', bytes);

	let winners: ValidatorIndex[] = winners_raw;
	let compact: CompactAssignments = compact_raw;
	let score: ElectionScore = score_raw;
	let era: EraIndex = era_raw;
	let size: ElectionSize = size_raw;

	let call = api.tx.staking.submitElectionSolution(winners, compact, score, era, size)
	let info = await api.rpc.payment.queryInfo(call.toJSON())
	const origin = keyring.addFromUri(readFileSync("key_real").toString().trim());

	console.log(`++ submitting call ${call.meta.name}, weight = ${info.weight.toHuman()}, partialFee = ${info.partialFee.toHuman()}`)
	console.log(`++ free balance of sender account (${origin.address}) = ${(await api.query.system.account(origin.address)).data.free.toHuman()}`)

	return 0;
	let exitCode = 0;

	try {
		let unsubscribe = await call.signAndSend(origin, ({ events = [], status, }) => {
			console.log(`Current status is ${status.type}`);

			if (status.isInBlock) {
				  console.log(`++ Transaction included at blockHash ${status.asInBlock}`);
			}

			if (status.isFinalized) {
				console.log(`++ Transaction Finalized at blockHash ${status.asFinalized}`);

				events.forEach(({ phase, event: { data, method, section } }) => {
					console.log(`\t' ${phase}: ${section}.${method}:: ${data}`);
				});
				unsubscribe();
			}

		})
	} catch(err) {
		exitCode = 1;
	}

	return exitCode
}


main().then(outcome => {
	process.exit(outcome)
})
.catch(err => {
	console.error("Unexpected error:", err)
});
