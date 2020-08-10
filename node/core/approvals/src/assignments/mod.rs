//! Approval checker assignments module
//!
//! Approval validity checks determine whether Polkadot considers a parachain candidate valid.
//! We distinguish them from backing validity checks that merely determine whether Polakdot
//! should begin processing a parachain candidate.


use std::collections::BTreeMap;

use polkadot_primitives::v1::{Id as ParaId, ValidatorId, Hash, Header};


use crate::Error;
pub type AssignmentResult<T> = Result<T,Error>;

pub mod stories;
pub mod criteria;
pub mod tracker;
pub mod announcer;


pub use stories::ApprovalContext;

pub type DelayTranche = u32;



/// Approvals target levels
///
/// We instantiuate this with `Default` currently, but we'll want the
/// relay VRF target number to be configurable by the chain eventually.
pub struct ApprovalTargets {
	/// Approvals required for criteria based upon relay chain VRF output,
	/// never too larger, never too small.
	pub relay_vrf_checkers: u32,
	/// Approvals required for criteria based upon relay chain equivocations,
	/// initially zero but increased if we discover equivocations.
	pub relay_equivocation_checkers: u32,
	/// How long we wait for no shows
	pub noshow_timeout: u32,
}

impl Default for ApprovalTargets {
	fn default() -> Self {
		ApprovalTargets {
			relay_vrf_checkers: 20,  // We've no analysis backing this choice yet.
			relay_equivocation_checkers: 0,
			noshow_timeout: 24, // Two relay chain blocks
		}
	}
}

impl ApprovalTargets {
	/// Target number of checkers by story type
	fn target<S: 'static>(&self) -> u32 {
		use core::any::TypeId;
		let s = TypeId::of::<S>();
		if s == TypeId::of::<stories::RelayVRFStory>()
			{ self.relay_vrf_checkers }
		else if s == TypeId::of::<stories::RelayEquivocationStory>()
			{ self.relay_equivocation_checkers }
		else { panic!("Oops, we've some foreign type for Criteria::Story!") }
	}
}


/// 
#[derive(Clone)]
pub struct AssigneeStatus {
	/// Highest tranche considered plus one
	tranche: DelayTranche,
	/// Assignement target, including increases due to no shows.
	pub target: u32,
	/// Assigned validators.
	pub assigned: u32,
	/// Awating approvals, including no shows.
	pub waiting: u32,
	/// Total no shows, including our debt.
	pub noshows: u32,
	/// Any no shows not yet addressed by additional tranches,
	/// often zero since adding extra tranches pays the debt fast.
	pub debt: u32,
	/// Approval votes thus far.
	pub approved: u32,
	/// How long we wait for no shows
	///
	/// Increases if we're replacing no shows from multiple tranches
	pub noshow_timeout: u32,
}

impl AssigneeStatus {
	/// Highest tranche considered thus far
	pub fn tranche(&self) -> Option<DelayTranche> {
		self.tranche.checked_sub(1)
	}

	pub fn is_approved(&self) -> bool {
		debug_assert!(self.assigned == self.assigned + self.waiting);
		self.target == self.approved && self.approved == self.assigned && self.waiting == 0
	}
}


