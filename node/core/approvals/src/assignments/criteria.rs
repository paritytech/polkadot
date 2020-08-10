//! Approval checker asignment VRF criteria
//!
//! We manage the actual VRF computations for approval checker
//! assignments inside this module, so most schnorrkell logic gets
//! isolated here.
//!
//! TODO: We should expand RelayVRFModulo to do rejection sampling
//! using `vrf::vrf_merge`, which requires `Vec<..>`s for
//! `AssignmentSigned::vrf_preout` and `Assignment::vrf_inout`.

use core::{borrow::Borrow, convert::TryFrom};

use merlin::Transcript;
use schnorrkel::{PublicKey, PUBLIC_KEY_LENGTH, Keypair, vrf};

// pub use sp_consensus_vrf::schnorrkel::{Randomness, VRF_PROOF_LENGTH, VRF_OUTPUT_LENGTH, RANDOMNESS_LENGTH };


use crate::Error;

use super::{
	ApprovalContext, AssignmentResult, Hash, ParaId, DelayTranche,
	stories, // RelayVRFStory, RelayEquivocationStory
	ValidatorId,
};


pub(super) fn validator_id_from_key(key: &PublicKey) -> ValidatorId {
	unimplemented!() // checker.public.to_bytes() except we need substrate's stuff here
}


impl ApprovalContext {
	pub fn transcript(&self) -> Transcript {
		let mut t = Transcript::new(b"Approval Assignment Signature");
		t.append_u64(b"rad slot", self.slot);
		t.append_u64(b"rad epoch", self.epoch);
		t.append_message(b"rad block", self.hash.as_ref());
		use primitives::crypto::Public;
		t.append_message(b"rad authority", &self.authority.to_raw_vec());  // Vec WTF?!?
		t
	}
}


/// Approval checker assignment criteria
/// 
/// We determine how the relay chain contet, any criteria data, and
/// any relevant stories impact VRF invokation using this trait,
pub trait Criteria : Clone + 'static {
	/// Additionl data required for constructing the VRF input
	type Story;

	/// Write the transcript from which build the VRF input.  
	///
	/// Cannot error unless `Criteria = RelayEquivocation`
	fn vrf_input(&self, story: &Self::Story) -> AssignmentResult<Transcript>;

	/// Initialize the transcript for our Schnorr DLEQ proof.
	///
	/// Any criteria data that requires authentication, which should make
	/// signing gossip messages unecessary, saving 64 bytes, etc.
	fn extra(&self, context: &ApprovalContext) -> Transcript { 
		context.transcript()
	}

	// fn position(&self, vrf_inout: &vrf::VRFInOut) -> Position;
}


/// Initial approval checker assignment based upon checkers' VRF 
/// applied to the relay chain VRF, but then computed modulo the
/// number of parachains.
#[derive(Clone)]
pub struct RelayVRFModulo {
	pub(crate) sample: u16,
	// Story::anv_rc_vrf_source
}

impl Criteria for RelayVRFModulo {
	type Story = stories::RelayVRFStory;

	/// Never errors.
	fn vrf_input(&self, story: &Self::Story) -> AssignmentResult<Transcript> {
		if self.sample > 0 { return Err(Error::BadAssignment("RelayVRFModulo does not yet support additional samples")); }
		let mut t = Transcript::new(b"Approval Assignment VRF");
		t.append_message(b"RelayVRF", &story.anv_rc_vrf_source );
		t.append_u64(b"Sample", self.sample.into() );
		Ok(t)
	}
}

// impl RelayVRFInitial { }


/// Approval checker assignment based upon checkers' VRF applied
/// to the relay chain VRF and parachain id, but then outputing a
/// delay.  Applies only if too few check before reaching the delay.
#[derive(Clone)]
pub struct RelayVRFDelay {
	// Story::anv_rc_vrf_source
	pub(crate) paraid: ParaId, 
}

impl Criteria for RelayVRFDelay {
	type Story = stories::RelayVRFStory;

	/// Never errors
	fn vrf_input(&self, story: &Self::Story) -> AssignmentResult<Transcript> {
		let mut t = Transcript::new(b"Approval Assignment VRF");
		t.append_message(b"RelayVRFDelay", &story.anv_rc_vrf_source );
		t.append_u64(b"ParaId", u32::from(self.paraid).into() );
		Ok(t)
	}
}

// impl RelayVRFDelay { }


/// Approval checker assignment based upon parablock hash
/// of a candidate equivocation.
#[derive(Clone)]
pub struct RelayEquivocation {
	// Story::anv_rc_vrf_source
	pub(crate) paraid: ParaId, 
}

impl Criteria for RelayEquivocation {
	type Story = stories::RelayEquivocationStory;

	/// Write the transcript from which build the VRF input for
	/// additional approval checks triggered by relay chain equivocations.
	///
	/// Errors if paraid does not yet count as a candidate equivocation 
	fn vrf_input(&self, story: &Self::Story) -> AssignmentResult<Transcript> {
		let h = story.candidate_equivocations.get(&self.paraid)
			.ok_or(Error::BadStory("Not a candidate equivocation")) ?;
		let mut t = Transcript::new(b"Approval Assignment VRF");
		t.append_u64(b"ParaId", u32::from(self.paraid).into() );
		t.append_message(b"Candidate Equivocation", h.as_ref() );
		Ok(t)
	}
}


/// Internal representation for a assigment with some computable
/// delay. 
/// We should obtain these first by verifying a signed
/// assignment using `AssignmentSigned::verify`, or simularly using
/// `Criteria::attach` manually, and secondly by evaluating our own
/// criteria.  In the later case, we produce a signed assignment
/// by calling `Assignment::sign`.
pub struct Assignment<C: Criteria, K = AssignmentSignature> {
	/// Assignment criteria specific data
	criteria: C,
	/// Assignment's VRF signature including its checker's key
	vrf_signature: K,
	/// VRFInOut from which we compute the actualy assignment details
	/// We could save some space by storing a `VRFPreOut` in
	/// `VRFSignature`, and storing some random output here.
	vrf_inout: vrf::VRFInOut,
}

impl<C> Assignment<C> where C: Criteria {
	/// Identify the checker
	pub fn checker(&self) -> &ValidatorId { &self.vrf_signature.checker }

	pub(super) fn checker_n_recieved(&self) -> (ValidatorId,DelayTranche)
		{ (self.vrf_signature.checker.clone(), self.vrf_signature.recieved) }

	/// Return our `AssignmentSigned`
	pub fn to_signed(&self, context: ApprovalContext) -> AssignmentSigned<C> {
		AssignmentSigned {
			context,
			criteria: self.criteria.clone(),
			checker: self.vrf_signature.checker.clone(),
			vrf_preout: self.vrf_inout.to_output().to_bytes(),
			vrf_proof: self.vrf_signature.vrf_proof.clone()
		}
	}
}

impl<C> Assignment<C,()> where C: Criteria {
	/// Create our own `Assignment` for the given criteria, story,
	/// and our keypair, by constructing its `VRFInOut`.
	pub fn create(criteria: C, story: &C::Story, checker: &Keypair) -> AssignmentResult<Assignment<C,()>> {
		let vrf_inout = checker.borrow().vrf_create_hash(criteria.vrf_input(story) ?);
		Ok(Assignment { criteria, vrf_signature: (), vrf_inout, })
	}

	/// VRF sign our assignment for announcment.
	///
	/// We could take `K: Borrow<Keypair>` above in `create`, saving us
	/// the `checker` argument here, and making `K=Arc<Keypair>` work,
	/// except `Assignment`s always occur with so much repetition that
	/// passing the `Keypair` again makes more sense.
	pub fn sign(
		&self,
		context: &ApprovalContext,
		checker: &Keypair,
		recieved: DelayTranche,
	) -> Assignment<C> {
		// Must exactly mirror `schnorrkel::Keypair::vrf_sign_extra`
		// or else rerun one point multiplicaiton in vrf_create_hash
		let t = self.criteria.extra(context);
		let vrf_proof = checker.dleq_proove(t, &self.vrf_inout, vrf::KUSAMA_VRF).0.to_bytes();
		let checker = validator_id_from_key(&checker.public);
		let vrf_signature = AssignmentSignature { checker, vrf_proof, recieved };
		Assignment { criteria: self.criteria.clone(), vrf_signature, vrf_inout: self.vrf_inout.clone(), }
	}
}


/// Assignment's VRF signature.  
#[derive(Clone)]
pub struct AssignmentSignature {
	/// Signer's public key
	checker: ValidatorId,
	/// DLEQ Proof of the VRF mapping the story to pre-output,
	/// and singing the context as well.
	vrf_proof: [u8; vrf::VRF_PROOF_LENGTH],
	/// Actual delay tranche when we recieved this announcement.
	///
	/// We never transfer this inside the `SignedAssignment`, but
	/// compute it outrelves when verifying assignment notices.  
	/// It prevents us counting late announcements as no shows too
	/// early, which permits delayed annoucements, such as caused
	/// by others' being no shows or to improve workload balancing.  
	recieved: DelayTranche,
}

/// Announcable VRF signed assignment
pub struct AssignmentSigned<C> {
	context: ApprovalContext,
	criteria: C,
	/// Signer's public key
	checker: ValidatorId,
	/// VRF Pre-Output computed from the story associated to this
	/// context and criteria, and proven correct by vrf_proof, but
	/// only yields output with `make_bytes` calls on its `VRFInOut`.
	vrf_preout: [u8; vrf::VRF_OUTPUT_LENGTH],
	/// DLEQ Proof of the VRF mapping the story to pre-output,
	/// and singing the context as well.
	vrf_proof: [u8; vrf::VRF_PROOF_LENGTH],
}

impl<C: Criteria> AssignmentSigned<C> {
	/// Get checker identity
	pub fn checker(&self) -> &ValidatorId { &self.checker }

	/// Get publickey identifying checker
	fn checker_pk(&self) -> AssignmentResult<PublicKey> {
		use primitives::crypto::Public;
		PublicKey::from_bytes(&self.checker.to_raw_vec()) // Vec WTF?!?
		.map_err(|_| Error::BadAssignment("Bad VRF signature (bad publickey)"))
	}

	/// Verify a signed assignment
	pub fn verify(&self, story: &C::Story, recieved: DelayTranche)
	 -> AssignmentResult<(&ApprovalContext,Assignment<C,AssignmentSignature>)> 
	{
		let AssignmentSigned { context, criteria, vrf_preout, checker, vrf_proof, } = self;
		let vrf_signature = AssignmentSignature {
			checker: checker.clone(),
			vrf_proof: vrf_proof.clone(),
			recieved, 
		};
		let checker_pk = self.checker_pk() ?;
		let vrf_inout = vrf::VRFOutput::from_bytes(vrf_preout)
			.expect("length enforced statically")
			.attach_input_hash(&checker_pk, criteria.vrf_input(story) ?)
			.map_err(|_| Error::BadAssignment("Bad VRF signature (bad pre-output)")) ?;
		let vrf_proof = vrf::VRFProof::from_bytes(vrf_proof)
			.map_err(|_| Error::BadAssignment("Bad VRF signature (bad proof)")) ?;
		let t = criteria.extra(&context);
		let _ = checker_pk.dleq_verify(t, &vrf_inout, &vrf_proof, vrf::KUSAMA_VRF)
			.map_err(|_| Error::BadAssignment("Bad VRF signature (invalid)")) ?;
		Ok((context, Assignment { criteria: criteria.clone(), vrf_signature, vrf_inout, }))
	}

	pub(super) fn serialize(&self) -> Vec<u8> {
		unimplemented!()
	}
}


pub(crate) enum SomeCriteria {
	RelayVRFModulo(RelayVRFModulo),
	RelayVRFDelay(RelayVRFDelay),
	RelayEquivocation(RelayEquivocation),
}

impl AssignmentSigned<SomeCriteria> {
	pub(super) fn deserialize(buf: &[u8]) -> AssignmentSigned<SomeCriteria> {
		unimplemented!()
	}
}


/// We require `Assignment<C,K>` methods generic over `C`
/// that position this assignment inside the assignment tracker
///
/// We pass `ApprovalContext` into both methods for availability core
/// information.  We need each cores' paraid assignment for `paraid`
/// of course, but `delay_tranche` only requires the approximate
/// number of availability cores, so we might avoid passing it there
/// in future once that number solidifies.
pub(super) trait Position {
	/// Assignment's  our `ParaId` from allowed `ParaId` returnned by
	/// `stories::allowed_paraids`.
	fn paraid(&self, context: &ApprovalContext) -> Option<ParaId>;

	/// Always assign `RelayVRFModulo` the zeroth delay tranche
	fn delay_tranche(&self, context: &ApprovalContext) -> DelayTranche { 0 }
}

impl<K> Position for Assignment<RelayVRFModulo,K> {
	/// Assign our `ParaId` from allowed `ParaId` returnned by
	/// `stories::allowed_paraids`.
	fn paraid(&self, context: &ApprovalContext) -> Option<ParaId> {
		// TODO: Optimize accessing this from `ApprovalContext`
		let paraids = context.paraids_by_core();
		// We use u64 here to give a reasonable distribution modulo the number of parachains
		let mut core = u64::from_le_bytes(self.vrf_inout.make_bytes::<[u8; 8]>(b"core"));
		core %= paraids.len() as u64;  // assumes usize < u64
		paraids[core as usize]
	}

	/// Always assign `RelayVRFModulo` the zeroth delay tranche
	fn delay_tranche(&self, _context: &ApprovalContext) -> DelayTranche { 0 }
}


/// Approval checker assignment criteria that fully utilizes delays.
///
/// We require this helper trait to help unify the handling of  
/// `RelayVRFDelay` and `RelayEquivocation`.
pub trait DelayCriteria : Criteria {
	/// All delay based assignment criteria contain an explicit paraid
	fn paraid(&self) -> ParaId;
	/// We consolodate this many plus one delays at tranche zero, 
	/// ensuring they always run their checks.
	fn zeroth_delay_tranche_width() -> DelayTranche;
}
impl DelayCriteria for RelayVRFDelay {
	fn paraid(&self) -> ParaId { self.paraid }
	/// We do not techncially require delay tranche zero checkers here
	/// thanks to `RelayVRFModulo`, but they help us tune the expected
	/// checkers, and our simple impl for `Position::delay_tranche`
	/// imposes at least one tranche worth.
	///
	/// If security dictates more zeroth delay checkers then we prefer
	/// adding allocations by `RelayVRFModulo` instead.
	fn zeroth_delay_tranche_width() -> DelayTranche { 0 } // 1
}
impl DelayCriteria for RelayEquivocation {
	fn paraid(&self) -> ParaId { self.paraid }
	/// Assigns twelve tranches worth of checks into delay tranch zero,
	/// meaning they always check candidate equivocations.
	///
	/// We do need some consolodation at zero for `RelayEquivocation`.
	/// We considered some modulo condition using relay chain block hashes,
	/// except we're already slashing someone for equivocation, so being
	/// less efficent hurts less than the extra code complexity.
	fn zeroth_delay_tranche_width() -> DelayTranche { 11 } // 12
}

impl<C,K> Position for Assignment<C,K> where C: DelayCriteria {
	/// Assign our `ParaId` from the one explicitly stored, but error 
	/// if disallowed by `stories::allowed_paraids`.
	///
	/// Errors if the paraid is not declared available here.
	fn paraid(&self, context: &ApprovalContext) -> Option<ParaId> {
		use core::ops::Deref;
		let paraid = self.criteria.paraid();
		// TODO:  Speed up!  Cores are not sorted so no binary_search here
		if context.core_by_paraid(paraid).is_none() { return None; }
		Some(paraid)
	}

	/// Assign our delay using our VRF output
	fn delay_tranche(&self, context: &ApprovalContext) -> DelayTranche {
		let delay_tranche_modulus = context.num_delay_tranches() 
			.saturating_add( C::zeroth_delay_tranche_width() );
		// We use u64 here to give a reasonable distribution modulo the number of tranches
		let mut delay_tranche = u64::from_le_bytes(self.vrf_inout.make_bytes::<[u8; 8]>(b"tranche"));
		delay_tranche %= delay_tranche_modulus as u64;
		delay_tranche.saturating_sub(C::zeroth_delay_tranche_width() as u64) as u32
	}
}


