use crate::chain::{
	self, polkadot::runtime_types::pallet_election_provider_multi_phase::RoundSnapshot,
};
use crate::{chain::MinerMaxVotesPerVoter, prelude::*, Balancing, Solver};
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};
use frame_support::BoundedVec;
use pallet_election_provider_multi_phase::{SolutionOf, SolutionOrSnapshotSize};
use sp_npos_elections::{ElectionScore, VoteWeight};

pub(crate) type Snapshot = (
	Vec<(AccountId, VoteWeight, BoundedVec<AccountId, MinerMaxVotesPerVoter>)>,
	Vec<AccountId>,
	u32,
);

pub(crate) async fn mine_solution<M>(
	api: &chain::polkadot::RuntimeApi,
	hash: Option<Hash>,
	solver: Solver,
) -> Result<(SolutionOf<M>, ElectionScore, SolutionOrSnapshotSize), Error>
where
	M: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = crate::chain::MinerMaxVotesPerVoter>
		+ 'static,
	<M as MinerConfig>::Solution: Send + Sync,
{
	let (voters, targets, desired_targets) = snapshot(&api, hash).await?;

	match solver {
		Solver::SeqPhragmen { .. } => {
			//BalanceIterations::set(*iterations);
			Miner::<M>::mine_solution_with_snapshot::<
				SequentialPhragmen<AccountId, Perbill, Balancing>,
			>(voters, targets, desired_targets)
		},
		Solver::PhragMMS { .. } => {
			//BalanceIterations::set(*iterations);
			Miner::<M>::mine_solution_with_snapshot::<PhragMMS<AccountId, Perbill, Balancing>>(
				voters,
				targets,
				desired_targets,
			)
		},
	}
	.map_err(|e| Error::Other(format!("{:?}", e)))
}

pub(crate) async fn snapshot(
	api: &chain::polkadot::RuntimeApi,
	hash: Option<Hash>,
) -> Result<Snapshot, Error> {
	let RoundSnapshot { voters, targets } = api
		.storage()
		.election_provider_multi_phase()
		.snapshot(hash)
		.await?
		.unwrap_or_else(|| RoundSnapshot { voters: Vec::new(), targets: Vec::new() });

	let desired_targets = api
		.storage()
		.election_provider_multi_phase()
		.desired_targets(hash)
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
