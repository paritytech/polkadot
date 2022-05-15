use crate::{chain, prelude::*, Balancing, Solver};
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};
use pallet_election_provider_multi_phase::{SolutionOf, SolutionOrSnapshotSize};
use sp_npos_elections::{ElectionScore, VoteWeight};

pub(crate) type Snapshot = (Vec<(AccountId, VoteWeight, BoundedVec)>, Vec<AccountId>, u32);

macro_rules! mine_solution_for { ($runtime:tt) => {
	paste::paste! {
		/// The monitor command.
		pub(crate) async fn [<mine_solution_$runtime>](
			api: &chain::$runtime::RuntimeApi,
			hash: Option<Hash>,
			solver: Solver
		) -> Result<(SolutionOf<chain::$runtime::Config>, ElectionScore, SolutionOrSnapshotSize), Error> {

				let (voters, targets, desired_targets) = [<snapshot_$runtime>](&api, hash).await?;

				match solver {
					Solver::SeqPhragmen { .. } => {
						//BalanceIterations::set(*iterations);
						Miner::<chain::$runtime::Config>::mine_solution_with_snapshot::<
							SequentialPhragmen<AccountId, Perbill, Balancing>,
						>(voters, targets, desired_targets)
					},
					Solver::PhragMMS { .. } => {
						//BalanceIterations::set(*iterations);
						Miner::<chain::$runtime::Config>::mine_solution_with_snapshot::<PhragMMS<AccountId, Perbill, Balancing>>(
							voters,
							targets,
							desired_targets,
						)
					},
				}
				.map_err(|e| Error::Other(format!("{:?}", e)))
		}
}}}

macro_rules! snapshot_for { ($runtime:tt) => {
	paste::paste! {

	pub(crate) async fn [<snapshot_$runtime>](api: &chain::$runtime::RuntimeApi, hash: Option<Hash>) -> Result<Snapshot, Error> {
		use crate::chain::$runtime::epm::RoundSnapshot;

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
				let mut bounded_vec = BoundedVec::default();

				// If this fails just crash the task.
				bounded_vec.try_append(&mut c.0).unwrap();

				(a, b, bounded_vec)
			})
			.collect();

			Ok((voters, targets, desired_targets))
	}
}}}

mine_solution_for!(polkadot);
mine_solution_for!(kusama);
mine_solution_for!(westend);
snapshot_for!(polkadot);
snapshot_for!(kusama);
snapshot_for!(westend);
