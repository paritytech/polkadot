use crate::{chains::MinerMaxVotesPerVoter, prelude::*, Balancing, Solver};
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};
use frame_support::BoundedVec;
use pallet_election_provider_multi_phase::{SolutionOf, SolutionOrSnapshotSize};
use runtime::pallet_election_provider_multi_phase::RoundSnapshot;
use sp_npos_elections::{ElectionScore, VoteWeight};

pub(crate) type Snapshot = (
	Vec<(AccountId, VoteWeight, BoundedVec<AccountId, MinerMaxVotesPerVoter>)>,
	Vec<AccountId>,
	u32,
);

pub(crate) async fn mine_solution<M>(
	api: &RuntimeApi,
	hash: Option<Hash>,
	solver: Solver,
) -> Result<(SolutionOf<M>, ElectionScore, SolutionOrSnapshotSize), Error>
where
	M: MinerConfig<AccountId = AccountId, MaxVotesPerVoter = crate::chains::MinerMaxVotesPerVoter>
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

pub(crate) async fn snapshot(api: &RuntimeApi, hash: Option<Hash>) -> Result<Snapshot, Error> {
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

use crate::runtime::runtime_types::{
	node_runtime::NposSolution16 as SubxtNposSolution16,
	sp_arithmetic::per_things::PerU16 as SubxtPerU16,
};

fn to_subxt_per_u16(x: sp_runtime::PerU16) -> SubxtPerU16 {
	SubxtPerU16(x.deconstruct())
}

pub(crate) fn to_subxt_raw_solution(
	s: crate::chains::polkadot::NposSolution16,
) -> SubxtNposSolution16 {
	SubxtNposSolution16 {
		votes1: s.votes1,
		votes2: s
			.votes2
			.into_iter()
			.map(|(a, b, c)| {
				let (b1, b2) = b[0];
				let b2 = to_subxt_per_u16(b2);
				(a, (b1, b2), c)
			})
			.collect(),
		votes3: s
			.votes3
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 2] = [Default::default(); 2];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes4: s
			.votes4
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 3] = [Default::default(); 3];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes5: s
			.votes5
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 4] = [Default::default(); 4];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes6: s
			.votes6
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 5] = [Default::default(); 5];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes7: s
			.votes7
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 6] = [Default::default(); 6];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes8: s
			.votes8
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 7] = [Default::default(); 7];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes9: s
			.votes9
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 8] = [Default::default(); 8];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes10: s
			.votes10
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 9] = [Default::default(); 9];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes11: s
			.votes11
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 10] = [Default::default(); 10];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes12: s
			.votes12
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 11] = [Default::default(); 11];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes13: s
			.votes13
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 12] = [Default::default(); 12];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes14: s
			.votes14
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 13] = [Default::default(); 13];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes15: s
			.votes15
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 14] = [Default::default(); 14];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
		votes16: s
			.votes16
			.into_iter()
			.map(|(a, b, c)| {
				let mut bb: [(u16, SubxtPerU16); 15] = [Default::default(); 15];
				for (out, inp) in bb.iter_mut().zip(b.into_iter()) {
					*out = (inp.0, to_subxt_per_u16(inp.1));
				}
				(a, bb, c)
			})
			.collect(),
	}
}
