// Copyright 2017-2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! The implementation of the inclusion emulator for the 'staging' runtime version.
//!
//! # Overview
//!
//! A set of utilities for node-side code to emulate the logic the runtime uses for checking
//! parachain blocks in order to build prospective parachains that are produced ahead of the
//! relay chain. These utilities allow the node-side to predict, with high accuracy, what
//! the relay-chain will accept in the near future.
//!
//! This module has 2 key data types: [`Constraints`] and [`Fragment`]s. [`Constraints`]
//! exhaustively define the set of valid inputs and outputs to parachain execution. A [`Fragment`]
//! indicates a parachain block, anchored to the relay-chain at a particular relay-chain block,
//! known as the relay-parent.
//!
//! ## Fragment Validity
//!
//! Every relay-parent is implicitly associated with a unique set of [`Constraints`] that describe
//! the properties that must be true for a block to be included in a direct child of that block,
//! assuming there is no intermediate parachain block pending availability.
//!
//! However, the key factor that makes asynchronously-grown prospective chains
//! possible is the fact that the relay-chain accepts candidate blocks based on whether they
//! are valid under the constraints of the present moment, not based on whether they were
//! valid at the time of construction.
//!
//! As such, [`Fragment`]s are often, but not always constructed in such a way that they are
//! invalid at first and become valid later on, as the relay chain grows.
//!
//! # Usage
//!
//! It's expected that the users of this module will be building up trees of
//! [`Fragment`]s and consistently pruning and adding to the tree.
//!
//! ## Operating Constraints
//!
//! The *operating constraints* of a `Fragment` are the constraints with which that fragment
//! was intended to comply. The operating constraints are defined as the base constraints
//! of the relay-parent of the fragment modified by the cumulative modifications of all
//! fragments between the relay-parent and the current fragment.
//!
//! What the operating constraints are, in practice, is a prediction about the state of the
//! relay-chain in the future. The relay-chain is aware of some current state, and we want to
//! make an intelligent prediction about what might be accepted in the future based on
//! prior fragments that also exist off-chain.
//!
//! ## Fragment Trees
//!
//! As the relay-chain grows, some predictions come true and others come false.
//! And new predictions get made. These three changes correspond distinctly to the
//! 3 primary operations on fragment trees.
//!
//! A fragment tree is a mental model for thinking about a forking series of predictions
//! about a single parachain. There may be one or more fragment trees per parachain.
//!
//! In expectation, most parachains will have a plausibly-unique authorship method which means that
//! they should really be much closer to fragment-chains, maybe with an occasional fork.
//!
//! Avoiding fragment-tree blowup is beyond the scope of this module.
//!
//! ### Pruning Fragment Trees
//!
//! When the relay-chain advances, we want to compare the new constraints of that relay-parent to
//! the roots of the fragment trees we have. There are 3 cases:
//!
//! 1. The root fragment is still valid under the new constraints. In this case, we do nothing. This
//! is the "prediction still uncertain" case.
//!
//! 2. The root fragment is invalid under the new constraints because it has been subsumed by the
//! relay-chain. In this case, we can discard the root and split & re-root the fragment tree under
//! its descendents and compare to the new constraints again. This is the "prediction came true"
//! case.
//!
//! 3. The root fragment is invalid under the new constraints because a competing parachain block
//! has been included or it would never be accepted for some other reason. In this case we can
//! discard the entire fragment tree. This is the "prediction came false" case.
//!
//! This is all a bit of a simplification because it assumes that the relay-chain advances without
//! forks and is finalized instantly. In practice, the set of fragment-trees needs to be observable
//! from the perspective of a few different possible forks of the relay-chain and not pruned
//! too eagerly.
//!
//! Note that the fragments themselves don't need to change and the only thing we care about
//! is whether the predictions they represent are still valid.
//!
//! ### Extending Fragment Trees
//!
//! As predictions fade into the past, new ones should be stacked on top.
//!
//! Every new relay-chain block is an opportunity to make a new prediction about the future.
//! Higher-level logic should select the leaves of the fragment-trees to build upon or whether
//! to create a new fragment-tree.
//!
//! ### Code Upgrades
//!
//! Code upgrades are the main place where this emulation fails. The on-chain PVF upgrade scheduling
//! logic is very path-dependent and intricate so we just assume that code upgrades
//! can't be initiated and applied within a single fragment-tree. Fragment-trees aren't deep,
//! in practice and code upgrades are fairly rare. So what's likely to happen around code
//! upgrades is that the entire fragment-tree has to get discarded at some point.
//!
//! That means a few blocks of execution time lost, which is not a big deal for code upgrades
//! in practice at most once every few weeks.

use polkadot_primitives::vstaging::{
	BlockNumber, CandidateCommitments, CollatorId, CollatorSignature,
	Constraints as PrimitiveConstraints, Hash, HeadData, Id as ParaId, PersistedValidationData,
	UpgradeRestriction, ValidationCodeHash,
};
use std::{
	borrow::{Borrow, Cow},
	collections::HashMap,
};

/// Constraints on inbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundHrmpLimitations {
	/// An exhaustive set of all valid watermarks, sorted ascending
	pub valid_watermarks: Vec<BlockNumber>,
}

/// Constraints on outbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundHrmpChannelLimitations {
	/// The maximum bytes that can be written to the channel.
	pub bytes_remaining: usize,
	/// The maximum messages that can be written to the channel.
	pub messages_remaining: usize,
}

/// Constraints on the actions that can be taken by a new parachain
/// block. These limitations are implicitly associated with some particular
/// parachain, which should be apparent from usage.
#[derive(Debug, Clone, PartialEq)]
pub struct Constraints {
	/// The minimum relay-parent number accepted under these constraints.
	pub min_relay_parent_number: BlockNumber,
	/// The maximum Proof-of-Validity size allowed, in bytes.
	pub max_pov_size: usize,
	/// The maximum new validation code size allowed, in bytes.
	pub max_code_size: usize,
	/// The amount of UMP messages remaining.
	pub ump_remaining: usize,
	/// The amount of UMP bytes remaining.
	pub ump_remaining_bytes: usize,
	/// The maximum number of UMP messages allowed per candidate.
	pub max_ump_num_per_candidate: usize,
	/// Remaining DMP queue. Only includes sent-at block numbers.
	pub dmp_remaining_messages: Vec<BlockNumber>,
	/// The limitations of all registered inbound HRMP channels.
	pub hrmp_inbound: InboundHrmpLimitations,
	/// The limitations of all registered outbound HRMP channels.
	pub hrmp_channels_out: HashMap<ParaId, OutboundHrmpChannelLimitations>,
	/// The maximum number of HRMP messages allowed per candidate.
	pub max_hrmp_num_per_candidate: usize,
	/// The required parent head-data of the parachain.
	pub required_parent: HeadData,
	/// The expected validation-code-hash of this parachain.
	pub validation_code_hash: ValidationCodeHash,
	/// The code upgrade restriction signal as-of this parachain.
	pub upgrade_restriction: Option<UpgradeRestriction>,
	/// The future validation code hash, if any, and at what relay-parent
	/// number the upgrade would be minimally applied.
	pub future_validation_code: Option<(BlockNumber, ValidationCodeHash)>,
}

impl From<PrimitiveConstraints> for Constraints {
	fn from(c: PrimitiveConstraints) -> Self {
		Constraints {
			min_relay_parent_number: c.min_relay_parent_number,
			max_pov_size: c.max_pov_size as _,
			max_code_size: c.max_code_size as _,
			ump_remaining: c.ump_remaining as _,
			ump_remaining_bytes: c.ump_remaining_bytes as _,
			max_ump_num_per_candidate: c.max_ump_num_per_candidate as _,
			dmp_remaining_messages: c.dmp_remaining_messages,
			hrmp_inbound: InboundHrmpLimitations {
				valid_watermarks: c.hrmp_inbound.valid_watermarks,
			},
			hrmp_channels_out: c
				.hrmp_channels_out
				.into_iter()
				.map(|(para_id, limits)| {
					(
						para_id,
						OutboundHrmpChannelLimitations {
							bytes_remaining: limits.bytes_remaining as _,
							messages_remaining: limits.messages_remaining as _,
						},
					)
				})
				.collect(),
			max_hrmp_num_per_candidate: c.max_hrmp_num_per_candidate as _,
			required_parent: c.required_parent,
			validation_code_hash: c.validation_code_hash,
			upgrade_restriction: c.upgrade_restriction,
			future_validation_code: c.future_validation_code,
		}
	}
}

/// Kinds of errors that can occur when modifying constraints.
#[derive(Debug, Clone, PartialEq)]
pub enum ModificationError {
	/// The HRMP watermark is not allowed.
	DisallowedHrmpWatermark(BlockNumber),
	/// No such HRMP outbound channel.
	NoSuchHrmpChannel(ParaId),
	/// Too many messages submitted to HRMP channel.
	HrmpMessagesOverflow {
		/// The ID of the recipient.
		para_id: ParaId,
		/// The amount of remaining messages in the capacity of the channel.
		messages_remaining: usize,
		/// The amount of messages submitted to the channel.
		messages_submitted: usize,
	},
	/// Too many bytes submitted to HRMP channel.
	HrmpBytesOverflow {
		/// The ID of the recipient.
		para_id: ParaId,
		/// The amount of remaining bytes in the capacity of the channel.
		bytes_remaining: usize,
		/// The amount of bytes submitted to the channel.
		bytes_submitted: usize,
	},
	/// Too many messages submitted to UMP.
	UmpMessagesOverflow {
		/// The amount of remaining messages in the capacity of UMP.
		messages_remaining: usize,
		/// The amount of messages submitted to UMP.
		messages_submitted: usize,
	},
	/// Too many bytes submitted to UMP.
	UmpBytesOverflow {
		/// The amount of remaining bytes in the capacity of UMP.
		bytes_remaining: usize,
		/// The amount of bytes submitted to UMP.
		bytes_submitted: usize,
	},
	/// Too many messages processed from DMP.
	DmpMessagesUnderflow {
		/// The amount of messages waiting to be processed from DMP.
		messages_remaining: usize,
		/// The amount of messages processed.
		messages_processed: usize,
	},
	/// No validation code upgrade to apply.
	AppliedNonexistentCodeUpgrade,
}

impl Constraints {
	/// Check modifications against constraints.
	pub fn check_modifications(
		&self,
		modifications: &ConstraintModifications,
	) -> Result<(), ModificationError> {
		if let Some(HrmpWatermarkUpdate::Trunk(hrmp_watermark)) = modifications.hrmp_watermark {
			// head updates are always valid.
			if self.hrmp_inbound.valid_watermarks.iter().all(|w| w != &hrmp_watermark) {
				return Err(ModificationError::DisallowedHrmpWatermark(hrmp_watermark))
			}
		}

		for (id, outbound_hrmp_mod) in &modifications.outbound_hrmp {
			if let Some(outbound) = self.hrmp_channels_out.get(&id) {
				outbound.bytes_remaining.checked_sub(outbound_hrmp_mod.bytes_submitted).ok_or(
					ModificationError::HrmpBytesOverflow {
						para_id: *id,
						bytes_remaining: outbound.bytes_remaining,
						bytes_submitted: outbound_hrmp_mod.bytes_submitted,
					},
				)?;

				outbound
					.messages_remaining
					.checked_sub(outbound_hrmp_mod.messages_submitted)
					.ok_or(ModificationError::HrmpMessagesOverflow {
						para_id: *id,
						messages_remaining: outbound.messages_remaining,
						messages_submitted: outbound_hrmp_mod.messages_submitted,
					})?;
			} else {
				return Err(ModificationError::NoSuchHrmpChannel(*id))
			}
		}

		self.ump_remaining.checked_sub(modifications.ump_messages_sent).ok_or(
			ModificationError::UmpMessagesOverflow {
				messages_remaining: self.ump_remaining,
				messages_submitted: modifications.ump_messages_sent,
			},
		)?;

		self.ump_remaining_bytes.checked_sub(modifications.ump_bytes_sent).ok_or(
			ModificationError::UmpBytesOverflow {
				bytes_remaining: self.ump_remaining_bytes,
				bytes_submitted: modifications.ump_bytes_sent,
			},
		)?;

		self.dmp_remaining_messages
			.len()
			.checked_sub(modifications.dmp_messages_processed)
			.ok_or(ModificationError::DmpMessagesUnderflow {
				messages_remaining: self.dmp_remaining_messages.len(),
				messages_processed: modifications.dmp_messages_processed,
			})?;

		if self.future_validation_code.is_none() && modifications.code_upgrade_applied {
			return Err(ModificationError::AppliedNonexistentCodeUpgrade)
		}

		Ok(())
	}

	/// Apply modifications to these constraints. If this succeeds, it passes
	/// all sanity-checks.
	pub fn apply_modifications(
		&self,
		modifications: &ConstraintModifications,
	) -> Result<Self, ModificationError> {
		let mut new = self.clone();

		if let Some(required_parent) = modifications.required_parent.as_ref() {
			new.required_parent = required_parent.clone();
		}

		if let Some(ref hrmp_watermark) = modifications.hrmp_watermark {
			match new.hrmp_inbound.valid_watermarks.binary_search(&hrmp_watermark.watermark()) {
				Ok(pos) => {
					// Exact match, so this is OK in all cases.
					let _ = new.hrmp_inbound.valid_watermarks.drain(..pos + 1);
				},
				Err(pos) => match hrmp_watermark {
					HrmpWatermarkUpdate::Head(_) => {
						// Updates to Head are always OK.
						let _ = new.hrmp_inbound.valid_watermarks.drain(..pos);
					},
					HrmpWatermarkUpdate::Trunk(n) => {
						// Trunk update landing on disallowed watermark is not OK.
						return Err(ModificationError::DisallowedHrmpWatermark(*n))
					},
				},
			}
		}

		for (id, outbound_hrmp_mod) in &modifications.outbound_hrmp {
			if let Some(outbound) = new.hrmp_channels_out.get_mut(&id) {
				outbound.bytes_remaining = outbound
					.bytes_remaining
					.checked_sub(outbound_hrmp_mod.bytes_submitted)
					.ok_or(ModificationError::HrmpBytesOverflow {
						para_id: *id,
						bytes_remaining: outbound.bytes_remaining,
						bytes_submitted: outbound_hrmp_mod.bytes_submitted,
					})?;

				outbound.messages_remaining = outbound
					.messages_remaining
					.checked_sub(outbound_hrmp_mod.messages_submitted)
					.ok_or(ModificationError::HrmpMessagesOverflow {
						para_id: *id,
						messages_remaining: outbound.messages_remaining,
						messages_submitted: outbound_hrmp_mod.messages_submitted,
					})?;
			} else {
				return Err(ModificationError::NoSuchHrmpChannel(*id))
			}
		}

		new.ump_remaining = new.ump_remaining.checked_sub(modifications.ump_messages_sent).ok_or(
			ModificationError::UmpMessagesOverflow {
				messages_remaining: new.ump_remaining,
				messages_submitted: modifications.ump_messages_sent,
			},
		)?;

		new.ump_remaining_bytes = new
			.ump_remaining_bytes
			.checked_sub(modifications.ump_bytes_sent)
			.ok_or(ModificationError::UmpBytesOverflow {
				bytes_remaining: new.ump_remaining_bytes,
				bytes_submitted: modifications.ump_bytes_sent,
			})?;

		if modifications.dmp_messages_processed > new.dmp_remaining_messages.len() {
			return Err(ModificationError::DmpMessagesUnderflow {
				messages_remaining: new.dmp_remaining_messages.len(),
				messages_processed: modifications.dmp_messages_processed,
			})
		} else {
			new.dmp_remaining_messages =
				new.dmp_remaining_messages[modifications.dmp_messages_processed..].to_vec();
		}

		if modifications.code_upgrade_applied {
			new.validation_code_hash = new
				.future_validation_code
				.take()
				.ok_or(ModificationError::AppliedNonexistentCodeUpgrade)?
				.1;
		}

		Ok(new)
	}
}

/// Information about a relay-chain block.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayChainBlockInfo {
	/// The hash of the relay-chain block.
	pub hash: Hash,
	/// The number of the relay-chain block.
	pub number: BlockNumber,
	/// The storage-root of the relay-chain block.
	pub storage_root: Hash,
}

/// An update to outbound HRMP channels.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OutboundHrmpChannelModification {
	/// The number of bytes submitted to the channel.
	pub bytes_submitted: usize,
	/// The number of messages submitted to the channel.
	pub messages_submitted: usize,
}

/// An update to the HRMP Watermark.
#[derive(Debug, Clone, PartialEq)]
pub enum HrmpWatermarkUpdate {
	/// This is an update placing the watermark at the head of the chain,
	/// which is always legal.
	Head(BlockNumber),
	/// This is an update placing the watermark behind the head of the
	/// chain, which is only legal if it lands on a block where messages
	/// were queued.
	Trunk(BlockNumber),
}

impl HrmpWatermarkUpdate {
	fn watermark(&self) -> BlockNumber {
		match *self {
			HrmpWatermarkUpdate::Head(n) | HrmpWatermarkUpdate::Trunk(n) => n,
		}
	}
}

/// Modifications to constraints as a result of prospective candidates.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstraintModifications {
	/// The required parent head to build upon.
	pub required_parent: Option<HeadData>,
	/// The new HRMP watermark
	pub hrmp_watermark: Option<HrmpWatermarkUpdate>,
	/// Outbound HRMP channel modifications.
	pub outbound_hrmp: HashMap<ParaId, OutboundHrmpChannelModification>,
	/// The amount of UMP messages sent.
	pub ump_messages_sent: usize,
	/// The amount of UMP bytes sent.
	pub ump_bytes_sent: usize,
	/// The amount of DMP messages processed.
	pub dmp_messages_processed: usize,
	/// Whether a pending code upgrade has been applied.
	pub code_upgrade_applied: bool,
}

impl ConstraintModifications {
	/// The 'identity' modifications: these can be applied to
	/// any constraints and yield the exact same result.
	pub fn identity() -> Self {
		ConstraintModifications {
			required_parent: None,
			hrmp_watermark: None,
			outbound_hrmp: HashMap::new(),
			ump_messages_sent: 0,
			ump_bytes_sent: 0,
			dmp_messages_processed: 0,
			code_upgrade_applied: false,
		}
	}

	/// Stack other modifications on top of these.
	///
	/// This does no sanity-checking, so if `other` is garbage relative
	/// to `self`, then the new value will be garbage as well.
	///
	/// This is an addition which is not commutative.
	pub fn stack(&mut self, other: &Self) {
		if let Some(ref new_parent) = other.required_parent {
			self.required_parent = Some(new_parent.clone());
		}
		if let Some(ref new_hrmp_watermark) = other.hrmp_watermark {
			self.hrmp_watermark = Some(new_hrmp_watermark.clone());
		}

		for (id, mods) in &other.outbound_hrmp {
			let record = self.outbound_hrmp.entry(*id).or_default();
			record.messages_submitted += mods.messages_submitted;
			record.bytes_submitted += mods.bytes_submitted;
		}

		self.ump_messages_sent += other.ump_messages_sent;
		self.ump_bytes_sent += other.ump_bytes_sent;
		self.dmp_messages_processed += other.dmp_messages_processed;
		self.code_upgrade_applied |= other.code_upgrade_applied;
	}
}

/// The prospective candidate.
///
/// This comprises the key information that represent a candidate
/// without pinning it to a particular session. For example, everything
/// to do with the collator's signature and commitments are represented
/// here. But the erasure-root is not. This means that prospective candidates
/// are not correlated to any session in particular.
#[derive(Debug, Clone, PartialEq)]
pub struct ProspectiveCandidate<'a> {
	/// The commitments to the output of the execution.
	pub commitments: Cow<'a, CandidateCommitments>,
	/// The collator that created the candidate.
	pub collator: CollatorId,
	/// The signature of the collator on the payload.
	pub collator_signature: CollatorSignature,
	/// The persisted validation data used to create the candidate.
	pub persisted_validation_data: PersistedValidationData,
	/// The hash of the PoV.
	pub pov_hash: Hash,
	/// The validation code hash used by the candidate.
	pub validation_code_hash: ValidationCodeHash,
}

impl<'a> ProspectiveCandidate<'a> {
	fn into_owned(self) -> ProspectiveCandidate<'static> {
		ProspectiveCandidate { commitments: Cow::Owned(self.commitments.into_owned()), ..self }
	}

	/// Partially clone the prospective candidate, but borrow the
	/// parts which are potentially heavy.
	pub fn partial_clone(&self) -> ProspectiveCandidate {
		ProspectiveCandidate {
			commitments: Cow::Borrowed(self.commitments.borrow()),
			collator: self.collator.clone(),
			collator_signature: self.collator_signature.clone(),
			persisted_validation_data: self.persisted_validation_data.clone(),
			pov_hash: self.pov_hash,
			validation_code_hash: self.validation_code_hash,
		}
	}
}

#[cfg(test)]
impl ProspectiveCandidate<'static> {
	fn commitments_mut(&mut self) -> &mut CandidateCommitments {
		self.commitments.to_mut()
	}
}

/// Kinds of errors with the validity of a fragment.
#[derive(Debug, Clone, PartialEq)]
pub enum FragmentValidityError {
	/// The validation code of the candidate doesn't match the
	/// operating constraints.
	///
	/// Expected, Got
	ValidationCodeMismatch(ValidationCodeHash, ValidationCodeHash),
	/// The persisted-validation-data doesn't match.
	///
	/// Expected, Got
	PersistedValidationDataMismatch(PersistedValidationData, PersistedValidationData),
	/// The outputs of the candidate are invalid under the operating
	/// constraints.
	OutputsInvalid(ModificationError),
	/// New validation code size too big.
	///
	/// Max allowed, new.
	CodeSizeTooLarge(usize, usize),
	/// Relay parent too old.
	///
	/// Min allowed, current.
	RelayParentTooOld(BlockNumber, BlockNumber),
	/// Para is required to process at least one DMP message from the queue.
	DmpAdvancementRule,
	/// Too many messages upward messages submitted.
	UmpMessagesPerCandidateOverflow {
		/// The amount of messages a single candidate can submit.
		messages_allowed: usize,
		/// The amount of messages sent to all HRMP channels.
		messages_submitted: usize,
	},
	/// Too many messages submitted to all HRMP channels.
	HrmpMessagesPerCandidateOverflow {
		/// The amount of messages a single candidate can submit.
		messages_allowed: usize,
		/// The amount of messages sent to all HRMP channels.
		messages_submitted: usize,
	},
	/// Code upgrade not allowed.
	CodeUpgradeRestricted,
	/// HRMP messages are not ascending or are duplicate.
	///
	/// The `usize` is the index into the outbound HRMP messages of
	/// the candidate.
	HrmpMessagesDescendingOrDuplicate(usize),
}

/// A parachain fragment, representing another prospective parachain block.
///
/// This is a type which guarantees that the candidate is valid under the
/// operating constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment<'a> {
	/// The new relay-parent.
	relay_parent: RelayChainBlockInfo,
	/// The constraints this fragment is operating under.
	operating_constraints: Constraints,
	/// The core information about the prospective candidate.
	candidate: ProspectiveCandidate<'a>,
	/// Modifications to the constraints based on the outputs of
	/// the candidate.
	modifications: ConstraintModifications,
}

impl<'a> Fragment<'a> {
	/// Create a new fragment.
	///
	/// This fails if the fragment isn't in line with the operating
	/// constraints. That is, either its inputs or its outputs fail
	/// checks against the constraints.
	///
	/// This doesn't check that the collator signature is valid or
	/// whether the PoV is small enough.
	pub fn new(
		relay_parent: RelayChainBlockInfo,
		operating_constraints: Constraints,
		candidate: ProspectiveCandidate<'a>,
	) -> Result<Self, FragmentValidityError> {
		let modifications = {
			let commitments = &candidate.commitments;
			ConstraintModifications {
				required_parent: Some(commitments.head_data.clone()),
				hrmp_watermark: Some({
					if commitments.hrmp_watermark == relay_parent.number {
						HrmpWatermarkUpdate::Head(commitments.hrmp_watermark)
					} else {
						HrmpWatermarkUpdate::Trunk(commitments.hrmp_watermark)
					}
				}),
				outbound_hrmp: {
					let mut outbound_hrmp = HashMap::<_, OutboundHrmpChannelModification>::new();

					let mut last_recipient = None::<ParaId>;
					for (i, message) in commitments.horizontal_messages.iter().enumerate() {
						if let Some(last) = last_recipient {
							if last >= message.recipient {
								return Err(
									FragmentValidityError::HrmpMessagesDescendingOrDuplicate(i),
								)
							}
						}

						last_recipient = Some(message.recipient);
						let record = outbound_hrmp.entry(message.recipient).or_default();

						record.bytes_submitted += message.data.len();
						record.messages_submitted += 1;
					}

					outbound_hrmp
				},
				ump_messages_sent: commitments.upward_messages.len(),
				ump_bytes_sent: commitments.upward_messages.iter().map(|msg| msg.len()).sum(),
				dmp_messages_processed: commitments.processed_downward_messages as _,
				code_upgrade_applied: operating_constraints
					.future_validation_code
					.map_or(false, |(at, _)| relay_parent.number >= at),
			}
		};

		validate_against_constraints(
			&operating_constraints,
			&relay_parent,
			&candidate,
			&modifications,
		)?;

		Ok(Fragment { relay_parent, operating_constraints, candidate, modifications })
	}

	/// Access the relay parent information.
	pub fn relay_parent(&self) -> &RelayChainBlockInfo {
		&self.relay_parent
	}

	/// Access the operating constraints
	pub fn operating_constraints(&self) -> &Constraints {
		&self.operating_constraints
	}

	/// Access the underlying prospective candidate.
	pub fn candidate(&self) -> &ProspectiveCandidate<'a> {
		&self.candidate
	}

	/// Modifications to constraints based on the outputs of the candidate.
	pub fn constraint_modifications(&self) -> &ConstraintModifications {
		&self.modifications
	}

	/// Convert the fragment into an owned variant.
	pub fn into_owned(self) -> Fragment<'static> {
		Fragment { candidate: self.candidate.into_owned(), ..self }
	}

	/// Validate this fragment against some set of constraints
	/// instead of the operating constraints.
	pub fn validate_against_constraints(
		&self,
		constraints: &Constraints,
	) -> Result<(), FragmentValidityError> {
		validate_against_constraints(
			constraints,
			&self.relay_parent,
			&self.candidate,
			&self.modifications,
		)
	}
}

fn validate_against_constraints(
	constraints: &Constraints,
	relay_parent: &RelayChainBlockInfo,
	candidate: &ProspectiveCandidate,
	modifications: &ConstraintModifications,
) -> Result<(), FragmentValidityError> {
	let expected_pvd = PersistedValidationData {
		parent_head: constraints.required_parent.clone(),
		relay_parent_number: relay_parent.number,
		relay_parent_storage_root: relay_parent.storage_root,
		max_pov_size: constraints.max_pov_size as u32,
	};

	if expected_pvd != candidate.persisted_validation_data {
		return Err(FragmentValidityError::PersistedValidationDataMismatch(
			expected_pvd,
			candidate.persisted_validation_data.clone(),
		))
	}

	if constraints.validation_code_hash != candidate.validation_code_hash {
		return Err(FragmentValidityError::ValidationCodeMismatch(
			constraints.validation_code_hash,
			candidate.validation_code_hash,
		))
	}

	if relay_parent.number < constraints.min_relay_parent_number {
		return Err(FragmentValidityError::RelayParentTooOld(
			constraints.min_relay_parent_number,
			relay_parent.number,
		))
	}

	if candidate.commitments.new_validation_code.is_some() {
		match constraints.upgrade_restriction {
			None => {},
			Some(UpgradeRestriction::Present) =>
				return Err(FragmentValidityError::CodeUpgradeRestricted),
		}
	}

	let announced_code_size = candidate
		.commitments
		.new_validation_code
		.as_ref()
		.map_or(0, |code| code.0.len());

	if announced_code_size > constraints.max_code_size {
		return Err(FragmentValidityError::CodeSizeTooLarge(
			constraints.max_code_size,
			announced_code_size,
		))
	}

	if modifications.dmp_messages_processed == 0 {
		if constraints
			.dmp_remaining_messages
			.get(0)
			.map_or(false, |&msg_sent_at| msg_sent_at <= relay_parent.number)
		{
			return Err(FragmentValidityError::DmpAdvancementRule)
		}
	}

	if candidate.commitments.horizontal_messages.len() > constraints.max_hrmp_num_per_candidate {
		return Err(FragmentValidityError::HrmpMessagesPerCandidateOverflow {
			messages_allowed: constraints.max_hrmp_num_per_candidate,
			messages_submitted: candidate.commitments.horizontal_messages.len(),
		})
	}

	if candidate.commitments.upward_messages.len() > constraints.max_ump_num_per_candidate {
		return Err(FragmentValidityError::UmpMessagesPerCandidateOverflow {
			messages_allowed: constraints.max_ump_num_per_candidate,
			messages_submitted: candidate.commitments.upward_messages.len(),
		})
	}

	constraints
		.check_modifications(&modifications)
		.map_err(FragmentValidityError::OutputsInvalid)
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::vstaging::{
		CollatorPair, HorizontalMessages, OutboundHrmpMessage, ValidationCode,
	};
	use sp_application_crypto::Pair;

	#[test]
	fn stack_modifications() {
		let para_a = ParaId::from(1u32);
		let para_b = ParaId::from(2u32);
		let para_c = ParaId::from(3u32);

		let a = ConstraintModifications {
			required_parent: None,
			hrmp_watermark: None,
			outbound_hrmp: {
				let mut map = HashMap::new();
				map.insert(
					para_a,
					OutboundHrmpChannelModification { bytes_submitted: 100, messages_submitted: 5 },
				);

				map.insert(
					para_b,
					OutboundHrmpChannelModification { bytes_submitted: 100, messages_submitted: 5 },
				);

				map
			},
			ump_messages_sent: 6,
			ump_bytes_sent: 1000,
			dmp_messages_processed: 5,
			code_upgrade_applied: true,
		};

		let b = ConstraintModifications {
			required_parent: None,
			hrmp_watermark: None,
			outbound_hrmp: {
				let mut map = HashMap::new();
				map.insert(
					para_b,
					OutboundHrmpChannelModification { bytes_submitted: 100, messages_submitted: 5 },
				);

				map.insert(
					para_c,
					OutboundHrmpChannelModification { bytes_submitted: 100, messages_submitted: 5 },
				);

				map
			},
			ump_messages_sent: 6,
			ump_bytes_sent: 1000,
			dmp_messages_processed: 5,
			code_upgrade_applied: true,
		};

		let mut c = a.clone();
		c.stack(&b);

		assert_eq!(
			c,
			ConstraintModifications {
				required_parent: None,
				hrmp_watermark: None,
				outbound_hrmp: {
					let mut map = HashMap::new();
					map.insert(
						para_a,
						OutboundHrmpChannelModification {
							bytes_submitted: 100,
							messages_submitted: 5,
						},
					);

					map.insert(
						para_b,
						OutboundHrmpChannelModification {
							bytes_submitted: 200,
							messages_submitted: 10,
						},
					);

					map.insert(
						para_c,
						OutboundHrmpChannelModification {
							bytes_submitted: 100,
							messages_submitted: 5,
						},
					);

					map
				},
				ump_messages_sent: 12,
				ump_bytes_sent: 2000,
				dmp_messages_processed: 10,
				code_upgrade_applied: true,
			},
		);

		let mut d = ConstraintModifications::identity();
		d.stack(&a);
		d.stack(&b);

		assert_eq!(c, d);
	}

	fn make_constraints() -> Constraints {
		let para_a = ParaId::from(1u32);
		let para_b = ParaId::from(2u32);
		let para_c = ParaId::from(3u32);

		Constraints {
			min_relay_parent_number: 5,
			max_pov_size: 1000,
			max_code_size: 1000,
			ump_remaining: 10,
			ump_remaining_bytes: 1024,
			max_ump_num_per_candidate: 5,
			dmp_remaining_messages: Vec::new(),
			hrmp_inbound: InboundHrmpLimitations { valid_watermarks: vec![6, 8] },
			hrmp_channels_out: {
				let mut map = HashMap::new();

				map.insert(
					para_a,
					OutboundHrmpChannelLimitations { messages_remaining: 5, bytes_remaining: 512 },
				);

				map.insert(
					para_b,
					OutboundHrmpChannelLimitations {
						messages_remaining: 10,
						bytes_remaining: 1024,
					},
				);

				map.insert(
					para_c,
					OutboundHrmpChannelLimitations { messages_remaining: 1, bytes_remaining: 128 },
				);

				map
			},
			max_hrmp_num_per_candidate: 5,
			required_parent: HeadData::from(vec![1, 2, 3]),
			validation_code_hash: ValidationCode(vec![4, 5, 6]).hash(),
			upgrade_restriction: None,
			future_validation_code: None,
		}
	}

	#[test]
	fn constraints_disallowed_trunk_watermark() {
		let constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		modifications.hrmp_watermark = Some(HrmpWatermarkUpdate::Trunk(7));

		assert_eq!(
			constraints.check_modifications(&modifications),
			Err(ModificationError::DisallowedHrmpWatermark(7)),
		);

		assert_eq!(
			constraints.apply_modifications(&modifications),
			Err(ModificationError::DisallowedHrmpWatermark(7)),
		);
	}

	#[test]
	fn constraints_always_allow_head_watermark() {
		let constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		modifications.hrmp_watermark = Some(HrmpWatermarkUpdate::Head(7));

		assert!(constraints.check_modifications(&modifications).is_ok());

		let new_constraints = constraints.apply_modifications(&modifications).unwrap();
		assert_eq!(new_constraints.hrmp_inbound.valid_watermarks, vec![8]);
	}

	#[test]
	fn constraints_no_such_hrmp_channel() {
		let constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		let bad_para = ParaId::from(100u32);
		modifications.outbound_hrmp.insert(
			bad_para,
			OutboundHrmpChannelModification { bytes_submitted: 0, messages_submitted: 0 },
		);

		assert_eq!(
			constraints.check_modifications(&modifications),
			Err(ModificationError::NoSuchHrmpChannel(bad_para)),
		);

		assert_eq!(
			constraints.apply_modifications(&modifications),
			Err(ModificationError::NoSuchHrmpChannel(bad_para)),
		);
	}

	#[test]
	fn constraints_hrmp_messages_overflow() {
		let constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		let para_a = ParaId::from(1u32);
		modifications.outbound_hrmp.insert(
			para_a,
			OutboundHrmpChannelModification { bytes_submitted: 0, messages_submitted: 6 },
		);

		assert_eq!(
			constraints.check_modifications(&modifications),
			Err(ModificationError::HrmpMessagesOverflow {
				para_id: para_a,
				messages_remaining: 5,
				messages_submitted: 6,
			}),
		);

		assert_eq!(
			constraints.apply_modifications(&modifications),
			Err(ModificationError::HrmpMessagesOverflow {
				para_id: para_a,
				messages_remaining: 5,
				messages_submitted: 6,
			}),
		);
	}

	#[test]
	fn constraints_hrmp_bytes_overflow() {
		let constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		let para_a = ParaId::from(1u32);
		modifications.outbound_hrmp.insert(
			para_a,
			OutboundHrmpChannelModification { bytes_submitted: 513, messages_submitted: 1 },
		);

		assert_eq!(
			constraints.check_modifications(&modifications),
			Err(ModificationError::HrmpBytesOverflow {
				para_id: para_a,
				bytes_remaining: 512,
				bytes_submitted: 513,
			}),
		);

		assert_eq!(
			constraints.apply_modifications(&modifications),
			Err(ModificationError::HrmpBytesOverflow {
				para_id: para_a,
				bytes_remaining: 512,
				bytes_submitted: 513,
			}),
		);
	}

	#[test]
	fn constraints_ump_messages_overflow() {
		let constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		modifications.ump_messages_sent = 11;

		assert_eq!(
			constraints.check_modifications(&modifications),
			Err(ModificationError::UmpMessagesOverflow {
				messages_remaining: 10,
				messages_submitted: 11,
			}),
		);

		assert_eq!(
			constraints.apply_modifications(&modifications),
			Err(ModificationError::UmpMessagesOverflow {
				messages_remaining: 10,
				messages_submitted: 11,
			}),
		);
	}

	#[test]
	fn constraints_ump_bytes_overflow() {
		let constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		modifications.ump_bytes_sent = 1025;

		assert_eq!(
			constraints.check_modifications(&modifications),
			Err(ModificationError::UmpBytesOverflow {
				bytes_remaining: 1024,
				bytes_submitted: 1025,
			}),
		);

		assert_eq!(
			constraints.apply_modifications(&modifications),
			Err(ModificationError::UmpBytesOverflow {
				bytes_remaining: 1024,
				bytes_submitted: 1025,
			}),
		);
	}

	#[test]
	fn constraints_dmp_messages() {
		let mut constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		assert!(constraints.check_modifications(&modifications).is_ok());
		assert!(constraints.apply_modifications(&modifications).is_ok());

		modifications.dmp_messages_processed = 6;

		assert_eq!(
			constraints.check_modifications(&modifications),
			Err(ModificationError::DmpMessagesUnderflow {
				messages_remaining: 0,
				messages_processed: 6,
			}),
		);

		assert_eq!(
			constraints.apply_modifications(&modifications),
			Err(ModificationError::DmpMessagesUnderflow {
				messages_remaining: 0,
				messages_processed: 6,
			}),
		);

		constraints.dmp_remaining_messages = vec![1, 4, 8, 10];
		modifications.dmp_messages_processed = 2;
		assert!(constraints.check_modifications(&modifications).is_ok());
		let constraints = constraints
			.apply_modifications(&modifications)
			.expect("modifications are valid");

		assert_eq!(&constraints.dmp_remaining_messages, &[8, 10]);
	}

	#[test]
	fn constraints_nonexistent_code_upgrade() {
		let constraints = make_constraints();
		let mut modifications = ConstraintModifications::identity();
		modifications.code_upgrade_applied = true;

		assert_eq!(
			constraints.check_modifications(&modifications),
			Err(ModificationError::AppliedNonexistentCodeUpgrade),
		);

		assert_eq!(
			constraints.apply_modifications(&modifications),
			Err(ModificationError::AppliedNonexistentCodeUpgrade),
		);
	}

	fn make_candidate(
		constraints: &Constraints,
		relay_parent: &RelayChainBlockInfo,
	) -> ProspectiveCandidate<'static> {
		let collator_pair = CollatorPair::generate().0;
		let collator = collator_pair.public();

		let sig = collator_pair.sign(b"blabla".as_slice());

		ProspectiveCandidate {
			commitments: Cow::Owned(CandidateCommitments {
				upward_messages: Default::default(),
				horizontal_messages: Default::default(),
				new_validation_code: None,
				head_data: HeadData::from(vec![1, 2, 3, 4, 5]),
				processed_downward_messages: 0,
				hrmp_watermark: relay_parent.number,
			}),
			collator,
			collator_signature: sig,
			persisted_validation_data: PersistedValidationData {
				parent_head: constraints.required_parent.clone(),
				relay_parent_number: relay_parent.number,
				relay_parent_storage_root: relay_parent.storage_root,
				max_pov_size: constraints.max_pov_size as u32,
			},
			pov_hash: Hash::repeat_byte(1),
			validation_code_hash: constraints.validation_code_hash,
		}
	}

	#[test]
	fn fragment_validation_code_mismatch() {
		let relay_parent = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let constraints = make_constraints();
		let mut candidate = make_candidate(&constraints, &relay_parent);

		let expected_code = constraints.validation_code_hash;
		let got_code = ValidationCode(vec![9, 9, 9]).hash();

		candidate.validation_code_hash = got_code;

		assert_eq!(
			Fragment::new(relay_parent, constraints, candidate),
			Err(FragmentValidityError::ValidationCodeMismatch(expected_code, got_code,)),
		)
	}

	#[test]
	fn fragment_pvd_mismatch() {
		let relay_parent = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let relay_parent_b = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0b),
			storage_root: Hash::repeat_byte(0xee),
		};

		let constraints = make_constraints();
		let candidate = make_candidate(&constraints, &relay_parent);

		let expected_pvd = PersistedValidationData {
			parent_head: constraints.required_parent.clone(),
			relay_parent_number: relay_parent_b.number,
			relay_parent_storage_root: relay_parent_b.storage_root,
			max_pov_size: constraints.max_pov_size as u32,
		};

		let got_pvd = candidate.persisted_validation_data.clone();

		assert_eq!(
			Fragment::new(relay_parent_b, constraints, candidate),
			Err(FragmentValidityError::PersistedValidationDataMismatch(expected_pvd, got_pvd,)),
		);
	}

	#[test]
	fn fragment_code_size_too_large() {
		let relay_parent = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let constraints = make_constraints();
		let mut candidate = make_candidate(&constraints, &relay_parent);

		let max_code_size = constraints.max_code_size;
		candidate.commitments_mut().new_validation_code = Some(vec![0; max_code_size + 1].into());

		assert_eq!(
			Fragment::new(relay_parent, constraints, candidate),
			Err(FragmentValidityError::CodeSizeTooLarge(max_code_size, max_code_size + 1,)),
		);
	}

	#[test]
	fn fragment_relay_parent_too_old() {
		let relay_parent = RelayChainBlockInfo {
			number: 3,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let constraints = make_constraints();
		let candidate = make_candidate(&constraints, &relay_parent);

		assert_eq!(
			Fragment::new(relay_parent, constraints, candidate),
			Err(FragmentValidityError::RelayParentTooOld(5, 3,)),
		);
	}

	#[test]
	fn fragment_hrmp_messages_overflow() {
		let relay_parent = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let constraints = make_constraints();
		let mut candidate = make_candidate(&constraints, &relay_parent);

		let max_hrmp = constraints.max_hrmp_num_per_candidate;

		candidate
			.commitments_mut()
			.horizontal_messages
			.try_extend((0..max_hrmp + 1).map(|i| OutboundHrmpMessage {
				recipient: ParaId::from(i as u32),
				data: vec![1, 2, 3],
			}))
			.unwrap();

		assert_eq!(
			Fragment::new(relay_parent, constraints, candidate),
			Err(FragmentValidityError::HrmpMessagesPerCandidateOverflow {
				messages_allowed: max_hrmp,
				messages_submitted: max_hrmp + 1,
			}),
		);
	}

	#[test]
	fn fragment_dmp_advancement_rule() {
		let relay_parent = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let mut constraints = make_constraints();
		let mut candidate = make_candidate(&constraints, &relay_parent);

		// Empty dmp queue is ok.
		assert!(Fragment::new(relay_parent.clone(), constraints.clone(), candidate.clone()).is_ok());
		// Unprocessed message that was sent later is ok.
		constraints.dmp_remaining_messages = vec![relay_parent.number + 1];
		assert!(Fragment::new(relay_parent.clone(), constraints.clone(), candidate.clone()).is_ok());

		for block_number in 0..=relay_parent.number {
			constraints.dmp_remaining_messages = vec![block_number];

			assert_eq!(
				Fragment::new(relay_parent.clone(), constraints.clone(), candidate.clone()),
				Err(FragmentValidityError::DmpAdvancementRule),
			);
		}

		candidate.commitments.to_mut().processed_downward_messages = 1;
		assert!(Fragment::new(relay_parent, constraints, candidate).is_ok());
	}

	#[test]
	fn fragment_ump_messages_overflow() {
		let relay_parent = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let constraints = make_constraints();
		let mut candidate = make_candidate(&constraints, &relay_parent);

		let max_ump = constraints.max_ump_num_per_candidate;

		candidate
			.commitments
			.to_mut()
			.upward_messages
			.try_extend((0..max_ump + 1).map(|i| vec![i as u8]))
			.unwrap();

		assert_eq!(
			Fragment::new(relay_parent, constraints, candidate),
			Err(FragmentValidityError::UmpMessagesPerCandidateOverflow {
				messages_allowed: max_ump,
				messages_submitted: max_ump + 1,
			}),
		);
	}

	#[test]
	fn fragment_code_upgrade_restricted() {
		let relay_parent = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let mut constraints = make_constraints();
		let mut candidate = make_candidate(&constraints, &relay_parent);

		constraints.upgrade_restriction = Some(UpgradeRestriction::Present);
		candidate.commitments_mut().new_validation_code = Some(ValidationCode(vec![1, 2, 3]));

		assert_eq!(
			Fragment::new(relay_parent, constraints, candidate),
			Err(FragmentValidityError::CodeUpgradeRestricted),
		);
	}

	#[test]
	fn fragment_hrmp_messages_descending_or_duplicate() {
		let relay_parent = RelayChainBlockInfo {
			number: 6,
			hash: Hash::repeat_byte(0x0a),
			storage_root: Hash::repeat_byte(0xff),
		};

		let constraints = make_constraints();
		let mut candidate = make_candidate(&constraints, &relay_parent);

		candidate.commitments_mut().horizontal_messages = HorizontalMessages::truncate_from(vec![
			OutboundHrmpMessage { recipient: ParaId::from(0 as u32), data: vec![1, 2, 3] },
			OutboundHrmpMessage { recipient: ParaId::from(0 as u32), data: vec![4, 5, 6] },
		]);

		assert_eq!(
			Fragment::new(relay_parent.clone(), constraints.clone(), candidate.clone()),
			Err(FragmentValidityError::HrmpMessagesDescendingOrDuplicate(1)),
		);

		candidate.commitments_mut().horizontal_messages = HorizontalMessages::truncate_from(vec![
			OutboundHrmpMessage { recipient: ParaId::from(1 as u32), data: vec![1, 2, 3] },
			OutboundHrmpMessage { recipient: ParaId::from(0 as u32), data: vec![4, 5, 6] },
		]);

		assert_eq!(
			Fragment::new(relay_parent, constraints, candidate),
			Err(FragmentValidityError::HrmpMessagesDescendingOrDuplicate(1)),
		);
	}
}
