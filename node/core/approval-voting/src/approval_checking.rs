// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Utilities for checking whether a candidate has been approved under a given block.

use polkadot_node_primitives::approval::DelayTranche;
use bitvec::slice::BitSlice;
use bitvec::order::Lsb0 as BitOrderLsb0;

use crate::persisted_entries::{ApprovalEntry, CandidateEntry};
use crate::time::Tick;

/// The required tranches of assignments needed to determine whether a candidate is approved.
#[derive(Debug, PartialEq)]
pub enum RequiredTranches {
	/// All validators appear to be required, based on tranches already taken and remaining
	/// no-shows.
	All,
	/// More tranches required - We're awaiting more assignments.
	Pending {
		/// The highest considered delay tranche when counting assignments.
		considered: DelayTranche,
		/// The tick at which the next no-show, of the assignments counted, would occur.
		next_no_show: Option<Tick>,
		/// The amount of uncovered votes that need to be covered. These are either uncovered no-shows
		/// or the amount of assignments needed to reach the minimum required.
		uncovered: usize,
		/// The highest tranche to consider when looking to broadcast own assignment.
		/// This should be considered along with the clock drift to avoid broadcasting
		/// assignments that are before the local time.
		maximum_broadcast: DelayTranche,
		/// The clock drift, in ticks, to apply to the local clock when determining whether
		/// to broadcast an assignment or when to schedule a wakeup. The local clock should be treated
		/// as though it is `clock_drift` ticks earlier.
		clock_drift: Tick,
	},
	/// An exact number of required tranches and a number of no-shows. This indicates that
	/// at least the amount of `needed_approvals` are assigned and additionally all no-shows
	/// are covered.
	Exact {
		/// The tranche to inspect up to.
		needed: DelayTranche,
		/// The amount of missing votes that should be tolerated.
		tolerated_missing: usize,
 		/// When the next no-show would be, if any. This is used to schedule the next wakeup in the
		/// event that there are some assignments that don't have corresponding approval votes. If this
		/// is `None`, all assignments have approvals.
		next_no_show: Option<Tick>,
	}
}

/// Check the approval of a candidate.
pub fn check_approval(
	candidate: &CandidateEntry,
	approval: &ApprovalEntry,
	required: RequiredTranches,
) -> bool {
	match required {
		RequiredTranches::Pending { .. } => false,
		RequiredTranches::All => {
			let approvals = candidate.approvals();
			3 * approvals.count_ones() > 2 * approvals.len()
		}
		RequiredTranches::Exact { needed, tolerated_missing, .. } => {
			// whether all assigned validators up to `needed` less no_shows have approved.
			// e.g. if we had 5 tranches and 1 no-show, we would accept all validators in
			// tranches 0..=5 except for 1 approving. In that example, we also accept all
			// validators in tranches 0..=5 approving, but that would indicate that the
			// RequiredTranches value was incorrectly constructed, so it is not realistic.
			// If there are more missing approvals than there are no-shows, that indicates
			// that there are some assignments which are not yet no-shows, but may become
			// no-shows.

			let mut assigned_mask = approval.assignments_up_to(needed);
			let approvals = candidate.approvals();

			let n_assigned = assigned_mask.count_ones();

			// Filter the amount of assigned validators by those which have approved.
			assigned_mask &= approvals.iter().by_val();
			let n_approved = assigned_mask.count_ones();

			// note: the process of computing `required` only chooses `exact` if
			// that will surpass a minimum amount of checks.
			// shouldn't typically go above, since all no-shows are supposed to be covered.
			n_approved + tolerated_missing >= n_assigned
		}
	}
}

fn merge_next_no_show(
	a: Option<Tick>,
	b: Option<Tick>,
) -> Option<Tick> {
	match (a, b) {
		(None, None) => None,
		(None, Some(x)) | (Some(x), None) => Some(x),
		(Some(x), Some(y)) => Some(x.min(y)),
	}
}

/// Determine the amount of tranches of assignments needed to determine approval of a candidate.
pub fn tranches_to_approve(
	approval_entry: &ApprovalEntry,
	approvals: &BitSlice<BitOrderLsb0, u8>,
	tranche_now: DelayTranche,
	block_tick: Tick,
	no_show_duration: Tick,
	needed_approvals: usize,
) -> RequiredTranches {
	// This function progresses through a series of states while looping over the tranches
	// that we are aware of. First, we perform an initial count of the number of assignments
	// until we reach the number of needed assignments for approval. As we progress, we count the
	// number of no-shows in each tranche.
	//
	// Then, if there are any no-shows, we proceed into a series of subsequent states for covering
	// no-shows.
	//
	// We cover each no-show by a non-empty tranche, keeping track of the amount of further
	// no-shows encountered along the way. Once all of the no-shows we were previously aware
	// of are covered, we then progress to cover the no-shows we encountered while covering those,
	// and so on.
	#[derive(Debug)]
	struct State {
		/// The total number of assignments obtained.
		assignments: usize,
		/// The depth of no-shows we are currently covering.
		depth: usize,
		/// The amount of no-shows that have been covered at the previous or current depths.
		covered: usize,
		/// The amount of assignments that we are attempting to cover at this depth.
		///
		/// At depth 0, these are the initial needed approvals, and at other depths these
		/// are no-shows.
		covering: usize,
		/// The number of uncovered no-shows encountered at this depth. These will be the
		/// `covering` of the next depth.
		uncovered: usize,
		/// The next tick at which a no-show would occur, if any.
		next_no_show: Option<Tick>,
	}

	impl State {
		fn output(
			&self,
			tranche: DelayTranche,
			needed_approvals: usize,
			n_validators: usize,
			no_show_duration: Tick,
		) -> RequiredTranches {
			if self.assignments + self.covering + self.uncovered >= n_validators {
				return RequiredTranches::All;
			}

			// If we have enough assignments and all no-shows are covered, we have reached the number
			// of tranches that we need to have.
			if self.assignments > needed_approvals && (self.covering + self.uncovered) == 0 {
				return RequiredTranches::Exact {
					needed: tranche,
					tolerated_missing: self.covered,
					next_no_show: self.next_no_show,
				};
			}

			// We're pending more assignments and should look at more tranches.
			let clock_drift = self.clock_drift(no_show_duration);
			if self.depth == 0 {
				RequiredTranches::Pending {
					considered: tranche,
					next_no_show: self.next_no_show,
					uncovered: (needed_approvals.saturating_sub(self.assignments)) + self.uncovered,
					// during the initial assignment-gathering phase, we want to accept assignments
					// from any tranche. Note that honest validators will still not broadcast their
					// assignment until it is time to do so, regardless of this value.
					maximum_broadcast: DelayTranche::max_value(),
					clock_drift,
				}
			} else {
				RequiredTranches::Pending {
					considered: tranche,
					next_no_show: self.next_no_show,
					uncovered: self.covering + self.uncovered,
					maximum_broadcast: tranche + (self.covering + self.uncovered) as DelayTranche,
					clock_drift,
				}
			}
		}

		fn clock_drift(&self, no_show_duration: Tick) -> Tick {
			self.depth as Tick * no_show_duration
		}

		fn advance(
			&self,
			new_assignments: usize,
			new_no_shows: usize,
			next_no_show: Option<Tick>,
		) -> State {
			let new_covered = if self.depth == 0 {
				new_assignments
			} else {
				// When covering no-shows, we treat each non-empty tranche as covering 1 assignment,
				// regardless of how many assignments are within the tranche.
				new_assignments.min(1)
			};

			let assignments = self.assignments + new_assignments;
			let covering = self.covering.saturating_sub(new_covered);
			let covered = self.covered + new_covered.min(self.covering); // don't 'over-cover'
			let uncovered = self.uncovered + new_no_shows;
			let next_no_show = merge_next_no_show(self.next_no_show, next_no_show);

			let (depth, covering, uncovered) = if covering == 0 {
				if uncovered == 0 {
					(self.depth, 0, self.uncovered)
				} else {
					(self.depth + 1, self.uncovered, 0)
				}
			} else {
				(self.depth, covering, uncovered)
			};

			State { assignments, depth, covered, covering, uncovered, next_no_show }
		}
	}

	let tick_now = tranche_now as Tick + block_tick;
	let n_validators = approval_entry.n_validators();

	let initial_state = State {
		assignments: 0,
		depth: 0,
		covered: 0,
		covering: needed_approvals,
		uncovered: 0,
		next_no_show: None,
	};

	// The `ApprovalEntry` doesn't have any data for empty tranches. We still want to iterate over
	// these empty tranches, so we create an iterator to fill the gaps.
	//
	// This iterator has an infinitely long amount of non-empty tranches appended to the end.
	let tranches_with_gaps_filled = {
		let mut gap_end = 0;
		let infinite_gap_start = approval_entry.tranches().last().map_or(0, |t| t.tranche() + 1);

		approval_entry.tranches()
			.iter()
			.flat_map(move |tranche_entry| {
				let tranche = tranche_entry.tranche();
				let assignments = tranche_entry.assignments();

				let gap_start = gap_end + 1;
				gap_end = tranche;

				(gap_start..tranche).map(|i| (i, &[] as &[_]))
					.chain(std::iter::once((tranche, assignments)))
			})
			.chain((infinite_gap_start..).map(|i| (i, &[] as &[_])))
	};

	tranches_with_gaps_filled
		.scan(Some(initial_state), |state, (tranche, assignments)| {
			// The `Option` here is used for early exit.
			let s = match state.take() {
				None => return None,
				Some(s) => s,
			};

			let clock_drift = s.clock_drift(no_show_duration);
			let drifted_tick_now = tick_now.saturating_sub(clock_drift);
			let drifted_tranche_now = drifted_tick_now.saturating_sub(block_tick) as DelayTranche;

			// Break the loop once we've taken enough tranches.
			// Note that we always take tranche 0 as `drifted_tranche_now` cannot be less than 0.
			if tranche > drifted_tranche_now {
				return None;
			}

			let n_assignments = assignments.len();

			// count no-shows. An assignment is a no-show if there is no corresponding approval vote
			// after a fixed duration.
			//
			// While we count the no-shows, we also determine the next possible no-show we might
			// see within this tranche.
			let mut next_no_show = None;
			let no_shows = {
				let next_no_show = &mut next_no_show;
				assignments.iter()
					.map(|(v_index, tick)| (v_index, tick + clock_drift + no_show_duration))
					.filter(|&(v_index, no_show_at)| {
						let has_approved = approvals.get(*v_index as usize).map(|b| !*b).unwrap_or(true);
						let is_no_show = !has_approved && no_show_at >= tick_now;

						if !is_no_show && !has_approved {
							*next_no_show = merge_next_no_show(*next_no_show, Some(no_show_at));
						}

						is_no_show
					}).count()
			};

			let s = s.advance(n_assignments, no_shows, next_no_show);
			let output = s.output(tranche, needed_approvals, n_validators, no_show_duration);

			*state = match output {
				RequiredTranches::Exact { .. } | RequiredTranches::All => {
					// Wipe the state clean so the next iteration of this closure will terminate
					// the iterator. This guarantees that we can call `last` further down to see
					// either a `Finished` or `Pending` result
					None
				}
				RequiredTranches::Pending { .. } => {
					// Pending results are only interesting when they are the last result of the iterator
					// i.e. we never achieve a satisfactory level of assignment.
					Some(s)
				}
			};

			Some(output)
		})
		.last()
		.expect("the underlying iterator is infinite and never exits early before tranche 1; qed")
}

#[cfg(test)]
mod tests {
	use super::*;

	use polkadot_primitives::v1::GroupIndex;
	use bitvec::bitvec;
	use bitvec::order::Lsb0 as BitOrderLsb0;

	use crate::approval_db;

	#[test]
	fn pending_is_not_approved() {
		let candidate = approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 0,
			block_assignments: Default::default(),
			approvals: Default::default(),
		}.into();

		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			assignments: Default::default(),
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();

		assert!(!check_approval(&candidate, &approval_entry, RequiredTranches::Pending(0)));
	}

	#[test]
	fn all_requires_supermajority() {
		let mut candidate: CandidateEntry = approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 0,
			block_assignments: Default::default(),
			approvals: bitvec![BitOrderLsb0, u8; 0; 10],
		}.into();

		for i in 0..6 {
			candidate.mark_approval(i);
		}

		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			assignments: bitvec![BitOrderLsb0, u8; 1; 10],
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();

		assert!(!check_approval(&candidate, &approval_entry, RequiredTranches::All));

		candidate.mark_approval(6);
		assert!(check_approval(&candidate, &approval_entry, RequiredTranches::All));
	}

	#[test]
	fn exact_takes_only_assignments_up_to() {
		let mut candidate: CandidateEntry = approval_db::v1::CandidateEntry {
			candidate: Default::default(),
			session: 0,
			block_assignments: Default::default(),
			approvals: bitvec![BitOrderLsb0, u8; 0; 10],
		}.into();

		for i in 0..6 {
			candidate.mark_approval(i);
		}

		let approval_entry = approval_db::v1::ApprovalEntry {
			tranches: vec![
				approval_db::v1::TrancheEntry {
					tranche: 0,
					assignments: (0..4).map(|i| (i, 0.into())).collect(),
				},
				approval_db::v1::TrancheEntry {
					tranche: 1,
					assignments: (4..6).map(|i| (i, 1.into())).collect(),
				},
				approval_db::v1::TrancheEntry {
					tranche: 2,
					assignments: (6..10).map(|i| (i, 0.into())).collect(),
				},
			],
			assignments: bitvec![BitOrderLsb0, u8; 1; 10],
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();

		assert!(check_approval(&candidate, &approval_entry, RequiredTranches::Exact(1, 0)));
		assert!(!check_approval(&candidate, &approval_entry, RequiredTranches::Exact(2, 0)));
		assert!(check_approval(&candidate, &approval_entry, RequiredTranches::Exact(2, 4)));
	}

	#[test]
	fn tranches_to_approve_everyone_present() {
		let block_tick = 0;
		let no_show_duration = 10;
		let needed_approvals = 4;

		let mut approval_entry: ApprovalEntry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			assignments: bitvec![BitOrderLsb0, u8; 0; 4],
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();

		approval_entry.import_assignment(0, 0, block_tick);
		approval_entry.import_assignment(0, 1, block_tick);

		approval_entry.import_assignment(1, 2, block_tick + 1);
		approval_entry.import_assignment(1, 3, block_tick + 1);

		let approvals = bitvec![BitOrderLsb0, u8; 1; 4];

		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				2,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Exact(1, 0),
		);
	}

	#[test]
	fn tranches_to_approve_no_shows_before_initial_count() {
		let block_tick = 20;
		let no_show_duration = 10;
		let needed_approvals = 4;

		let mut approval_entry: ApprovalEntry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			assignments: bitvec![BitOrderLsb0, u8; 0; 4],
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();

		approval_entry.import_assignment(0, 0, block_tick);
		approval_entry.import_assignment(0, 1, block_tick);

		approval_entry.import_assignment(1, 2, block_tick);

		let mut approvals = bitvec![BitOrderLsb0, u8; 0; 4];
		approvals.set(0, true);
		approvals.set(1, true);

		let tranche_now = no_show_duration as DelayTranche + 1;
		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				tranche_now,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Pending(tranche_now),
		);
	}

	#[test]
	fn tranches_to_approve_not_enough_initial() {
		let block_tick = 0;
		let no_show_duration = 10;
		let needed_approvals = 4;

		let mut approval_entry: ApprovalEntry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			assignments: bitvec![BitOrderLsb0, u8; 0; 4],
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();

		approval_entry.import_assignment(0, 0, block_tick);
		approval_entry.import_assignment(0, 1, block_tick);

		approval_entry.import_assignment(1, 3, block_tick + 1);

		let approvals = bitvec![BitOrderLsb0, u8; 1; 4];

		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				8,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Pending(8), // tranche_now
		);
	}

	#[test]
	fn tranches_to_approve_cover_no_show_not_enough() {
		let block_tick = 20;
		let no_show_duration = 10;
		let needed_approvals = 4;
		let n_validators = 8;

		let mut approval_entry: ApprovalEntry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			assignments: bitvec![BitOrderLsb0, u8; 0; n_validators],
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();

		approval_entry.import_assignment(0, 0, block_tick);
		approval_entry.import_assignment(0, 1, block_tick);

		approval_entry.import_assignment(1, 2, block_tick);
		approval_entry.import_assignment(1, 3, block_tick);

		let mut approvals = bitvec![BitOrderLsb0, u8; 0; n_validators];
		approvals.set(0, true);
		approvals.set(1, true);
		// skip 2
		approvals.set(3, true);

		let tranche_now = no_show_duration as DelayTranche + 1;
		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				tranche_now,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Pending(2), // tranche 1 + 1 no-show.
		);

		approvals.set(0, false);

		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				tranche_now,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Pending(3), // tranche 1 + 2 no-show.
		);
	}

	#[test]
	fn tranches_to_approve_multi_cover_not_enough() {
		let block_tick = 20;
		let no_show_duration = 10;
		let needed_approvals = 4;
		let n_validators = 8;

		let mut approval_entry: ApprovalEntry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			assignments: bitvec![BitOrderLsb0, u8; 0; n_validators],
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();


		approval_entry.import_assignment(0, 0, block_tick);
		approval_entry.import_assignment(0, 1, block_tick);

		approval_entry.import_assignment(1, 2, block_tick);
		approval_entry.import_assignment(1, 3, block_tick);

		approval_entry.import_assignment(2, 4, block_tick);
		approval_entry.import_assignment(2, 5, block_tick);

		let mut approvals = bitvec![BitOrderLsb0, u8; 0; n_validators];
		approvals.set(0, true);
		approvals.set(1, true);
		// skip 2
		approvals.set(3, true);
		// skip 4
		approvals.set(5, true);

		let tranche_now = no_show_duration as DelayTranche + 1;
		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				tranche_now,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Pending(3), // tranche 2 + 1 uncovered no-show
		);
	}

	#[test]
	fn tranches_to_approve_cover_no_show() {
		let block_tick = 20;
		let no_show_duration = 10;
		let needed_approvals = 4;
		let n_validators = 8;

		let mut approval_entry: ApprovalEntry = approval_db::v1::ApprovalEntry {
			tranches: Vec::new(),
			assignments: bitvec![BitOrderLsb0, u8; 0; n_validators],
			our_assignment: None,
			backing_group: GroupIndex(0),
			approved: false,
		}.into();

		approval_entry.import_assignment(0, 0, block_tick);
		approval_entry.import_assignment(0, 1, block_tick);

		approval_entry.import_assignment(1, 2, block_tick);
		approval_entry.import_assignment(1, 3, block_tick);

		approval_entry.import_assignment(2, 4, block_tick);
		approval_entry.import_assignment(2, 5, block_tick);

		let mut approvals = bitvec![BitOrderLsb0, u8; 0; n_validators];
		approvals.set(0, true);
		approvals.set(1, true);
		// skip 2
		approvals.set(3, true);
		approvals.set(4, true);
		approvals.set(5, true);

		let tranche_now = no_show_duration as DelayTranche + 1;
		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				tranche_now,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Exact(2, 1),
		);

		// Even though tranche 2 has 2 validators, it only covers 1 no-show.
		// to cover a second no-show, we need to take another non-empty tranche.

		approvals.set(0, false);

		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				tranche_now,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Pending(3),
		);

		approval_entry.import_assignment(3, 6, block_tick);
		approvals.set(6, true);

		assert_eq!(
			tranches_to_approve(
				&approval_entry,
				&approvals,
				tranche_now,
				block_tick,
				no_show_duration,
				needed_approvals,
			),
			RequiredTranches::Exact(3, 2),
		);
	}
}
