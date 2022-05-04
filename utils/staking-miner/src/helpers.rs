use crate::{
	prelude::*, runtime::runtime_types::pallet_election_provider_multi_phase::RoundSnapshot,
	BalanceIterations, Balancing, MonitorConfig, Solver, SubmissionStrategy,
};
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};
use frame_support::BoundedVec;
use pallet_election_provider_multi_phase::{
	Miner, RawSolution, SolutionOf, SolutionOrSnapshotSize,
};
use sp_npos_elections::{ElectionScore, VoteWeight};
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::{sp_core::storage::StorageKey, TransactionStatus};

pub(crate) async fn mine_solution_polkadot(
	api: &RuntimeApi,
	hash: Hash,
	solver: Solver,
) -> Result<(SolutionOf<polkadot::Config>, ElectionScore, SolutionOrSnapshotSize), Error> {
	let (voters, targets, desired_targets) = snapshot(api, hash).await?;

	match solver {
		Solver::SeqPhragmen { iterations } => PolkadotMiner::mine_solution_with_snapshot::<
			SequentialPhragmen<AccountId, Perbill, Balancing>,
		>(voters, targets, desired_targets),
		Solver::PhragMMS { iterations } => PolkadotMiner::mine_solution_with_snapshot::<
			PhragMMS<AccountId, Perbill, Balancing>,
		>(voters, targets, desired_targets),
	}
	.map_err(|e| Error::Other(format!("{:?}", e)))
}

pub(crate) async fn mine_solution_kusama(
	api: &RuntimeApi,
	hash: Hash,
	solver: Solver,
) -> Result<(SolutionOf<kusama::Config>, ElectionScore, SolutionOrSnapshotSize), Error> {
	let (voters, targets, desired_targets) = snapshot(api, hash).await?;

	match solver {
		Solver::SeqPhragmen { iterations } => KusamaMiner::mine_solution_with_snapshot::<
			SequentialPhragmen<AccountId, Perbill, Balancing>,
		>(voters, targets, desired_targets),
		Solver::PhragMMS { iterations } => KusamaMiner::mine_solution_with_snapshot::<
			PhragMMS<AccountId, Perbill, Balancing>,
		>(voters, targets, desired_targets),
	}
	.map_err(|e| Error::Other(format!("{:?}", e)))
}

type Snapshot = (
	Vec<(AccountId, VoteWeight, BoundedVec<AccountId, MinerMaxVotesPerVotes>)>,
	Vec<AccountId>,
	u32,
);

async fn snapshot(api: &RuntimeApi, hash: Hash) -> Result<Snapshot, Error> {
	let RoundSnapshot { voters, targets } = api
		.storage()
		.election_provider_multi_phase()
		.snapshot(Some(hash))
		.await?
		.unwrap_or_else(|| RoundSnapshot { voters: Vec::new(), targets: Vec::new() });

	let desired_targets = api
		.storage()
		.election_provider_multi_phase()
		.desired_targets(Some(hash))
		.await?
		.unwrap_or_default();

	let voters: Vec<_> = voters
		.into_iter()
		.map(|(a, b, mut c)| {
			let mut bounded_vec: BoundedVec<_, MinerMaxVotesPerVotes> =
				frame_support::BoundedVec::default();

			// If this fails just crash the task.
			bounded_vec.try_append(&mut c.0).unwrap();

			(a, b, bounded_vec)
		})
		.collect();

	Ok((voters, targets, desired_targets))
}
