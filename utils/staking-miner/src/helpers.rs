use crate::{
	chains::MinerMaxVotesPerVoter, prelude::*,
	runtime::runtime_types::pallet_election_provider_multi_phase::RoundSnapshot, BalanceIterations,
	Balancing, MonitorConfig, Solver, SubmissionStrategy,
};
use frame_support::BoundedVec;
use pallet_election_provider_multi_phase::{
	Miner, RawSolution, SolutionOf, SolutionOrSnapshotSize,
};
use sp_npos_elections::{ElectionScore, VoteWeight};
use sp_runtime::Perbill;
use std::sync::Arc;
use subxt::{sp_core::storage::StorageKey, TransactionStatus};

pub(crate) type Snapshot = (
	Vec<(AccountId, VoteWeight, BoundedVec<AccountId, MinerMaxVotesPerVoter>)>,
	Vec<AccountId>,
	u32,
);

pub(crate) async fn snapshot(api: &RuntimeApi, hash: Hash) -> Result<Snapshot, Error> {
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
			let mut bounded_vec: BoundedVec<_, MinerMaxVotesPerVoter> =
				frame_support::BoundedVec::default();

			// If this fails just crash the task.
			bounded_vec.try_append(&mut c.0).unwrap();

			(a, b, bounded_vec)
		})
		.collect();

	Ok((voters, targets, desired_targets))
}
