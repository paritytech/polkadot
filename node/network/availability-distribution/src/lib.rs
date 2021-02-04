
// Copyright 2021 Parity Technologies (UK) Ltd.
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

/// The bitfield distribution subsystem.
pub struct AvailabilityDistributionSubsystem {
	/// Pointer to a keystore, which is required for determining this nodes validator index.
	keystore: SyncCryptoStorePtr,
	/// Prometheus metrics.
	metrics: Metrics,
}

/// Metadata about a candidate that is part of the live_candidates set.
///
/// Those which were not present in a cache are "fresh" and have their candidate descriptor attached. This
/// information is propagated to the higher level where it can be used to create data entries. Cached candidates
/// already have entries associated with them, and thus don't need this metadata to be fetched.
#[derive(Debug)]
enum FetchedLiveCandidate {
	Cached,
	Fresh(CandidateDescriptor),
}

struct ProtocolState {
	/// Candidates we need to fetch our chunk for.
	chunks_to_fetch: HashMap<CandidateHash, CandidateDescriptor>,

	/// Localized information about sessions we are currently interested in.
	///
	/// This is usually the current one and at session boundaries also the last one.
	session_infos: HashMap<SessionIndex, SessionInfo>,

}

/// Localized session information, tailored for the needs of availability distribution.
struct SessionInfo {
	/// For each core we maintain a randomized list of corresponding validators.
	///
	/// This is so we can query them for chunks, trying them in order. As each validator will
	/// have a randomized ordering, we should get good load balancing.
	validator_groups: Vec<Vec<ValidatorIndex>>,
}

struct ChunkFetchingInfo {
	descriptor: CandidateDescriptor,
	/// Validators that backed the candidate and hopefully have our chunk.
	backing_group: Vec<ValidatorIndex>,
}

fn run() {
	/// Get current heads
	/// For each chunk/slot, update randomized list of validators to query on session bundaries.
	/// Fetch pending availability candidates and add them to `chunks_to_fetch`.
}

/// Obtain all live candidates under a particular relay head. This implicitly includes
/// `K` ancestors of the head, such that the candidates pending availability in all of
/// the states of the head and the ancestors are unioned together to produce the
/// return type of this function. Each candidate hash is paired with information about
/// from where it was fetched.
///
/// This also updates all `live_under` cached by the protocol state and returns a list
/// of up to `K` ancestors of the relay-parent.
#[tracing::instrument(level = "trace", skip(ctx, live_under), fields(subsystem = LOG_TARGET))]
async fn query_live_candidates<Context>(
	ctx: &mut Context,
	live_under: &mut HashMap<Hash, HashSet<CandidateHash>>,
	relay_parent: Hash,
) -> Result<(HashMap<CandidateHash, FetchedLiveCandidate>, Vec<Hash>)>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	// register one of relay parents (not the ancestors)
	let ancestors = query_up_to_k_ancestors_in_same_session(
		ctx,
		relay_parent,
		AvailabilityDistributionSubsystem::K,
	)
	.await?;

	// query the ones that were not present in the live_under cache and add them
	// to it.
	let live_candidates = query_pending_availability_at(
		ctx,
		ancestors.iter().cloned().chain(iter::once(relay_parent)),
		live_under,
	).await?;

	Ok((live_candidates, ancestors))
}

/// Obtain all live candidates for all given `relay_blocks`.
///
/// This returns a set of all candidate hashes pending availability within the state
/// of the explicitly referenced relay heads.
///
/// This also queries the provided `live_under` cache before reaching into the
/// runtime and updates it with the information learned.
#[tracing::instrument(level = "trace", skip(ctx, relay_blocks, live_under), fields(subsystem = LOG_TARGET))]
async fn query_pending_availability_at<Context>(
	ctx: &mut Context,
	relay_blocks: impl IntoIterator<Item = Hash>,
	live_under: &mut HashMap<Hash, HashSet<CandidateHash>>,
) -> Result<HashMap<CandidateHash, FetchedLiveCandidate>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let mut live_candidates = HashMap::new();

	// fetch and fill out cache for each of these
	for relay_parent in relay_blocks {
		let receipts_for = match live_under.entry(relay_parent) {
			Entry::Occupied(e) => {
				live_candidates.extend(
					e.get().iter().cloned().map(|c| (c, FetchedLiveCandidate::Cached))
				);
				continue
			},
			e => e.or_default(),
		};

		for (receipt_hash, descriptor) in query_pending_availability(ctx, relay_parent).await? {
			// unfortunately we have no good way of telling the candidate was
			// cached until now. But we don't clobber a `Cached` entry if there
			// is one already.
			live_candidates.entry(receipt_hash).or_insert(FetchedLiveCandidate::Fresh(descriptor));
			receipts_for.insert(receipt_hash);
		}
	}

	Ok(live_candidates)
}

/// Query all hashes and descriptors of candidates pending availability at a particular block.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_pending_availability<Context>(ctx: &mut Context, relay_parent: Hash)
	-> Result<Vec<(CandidateHash, CandidateDescriptor)>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::AvailabilityCores(tx),
	)))
	.await;

	let cores: Vec<_> = rx
		.await
		.map_err(|e| Error::AvailabilityCoresResponseChannel(e))?
		.map_err(|e| Error::AvailabilityCores(e))?;

	Ok(cores.into_iter()
		.filter_map(|core_state| if let CoreState::Occupied(occupied) = core_state {
			Some((occupied.candidate_hash, occupied.candidate_descriptor))
		} else {
			None
		})
		.collect())
}
