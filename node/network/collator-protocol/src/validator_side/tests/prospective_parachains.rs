// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Tests for the validator side with enabled prospective parachains.

use super::*;

use polkadot_node_subsystem::messages::ChainApiMessage;
use polkadot_primitives::v2::Header;

const API_VERSION_PROSPECTIVE_ENABLED: u32 = 3;

const ALLOWED_ANCESTRY: u32 = 3;

fn get_parent_hash(hash: Hash) -> Hash {
	Hash::from_low_u64_be(hash.to_low_u64_be() + 1)
}

/// Handle a view update.
async fn update_view(
	virtual_overseer: &mut VirtualOverseer,
	test_state: &TestState,
	new_view: Vec<(Hash, u32)>, // Hash and block number.
	activated: u8,              // How many new heads does this update contain?
) {
	let new_view: HashMap<Hash, u32> = HashMap::from_iter(new_view);

	let our_view =
		OurView::new(new_view.keys().map(|hash| (*hash, Arc::new(jaeger::Span::Disabled))), 0);

	overseer_send(
		virtual_overseer,
		CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(our_view)),
	)
	.await;

	let mut next_overseer_message = None;
	for _ in 0..activated {
		let (leaf_hash, leaf_number) = assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				parent,
				RuntimeApiRequest::Version(tx),
			)) => {
				tx.send(Ok(API_VERSION_PROSPECTIVE_ENABLED)).unwrap();
				(parent, new_view.get(&parent).copied().expect("Unknown parent requested"))
			}
		);

		let min_number = leaf_number.saturating_sub(ALLOWED_ANCESTRY);

		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::GetMinimumRelayParents(parent, tx),
			) if parent == leaf_hash => {
				tx.send(test_state.chain_ids.iter().map(|para_id| (*para_id, min_number)).collect()).unwrap();
			}
		);

		let ancestry_len = leaf_number + 1 - min_number;
		let ancestry_hashes = std::iter::successors(Some(leaf_hash), |h| Some(get_parent_hash(*h)))
			.take(ancestry_len as usize);
		let ancestry_numbers = (min_number..=leaf_number).rev();
		let ancestry_iter = ancestry_hashes.clone().zip(ancestry_numbers).peekable();

		// How many blocks were actually requested.
		let mut requested_len = 0;
		{
			let mut ancestry_iter = ancestry_iter.clone();
			loop {
				let (hash, number) = match ancestry_iter.next() {
					Some((hash, number)) => (hash, number),
					None => break,
				};

				// May be `None` for the last element.
				let parent_hash =
					ancestry_iter.peek().map(|(h, _)| *h).unwrap_or_else(|| get_parent_hash(hash));

				let msg = match next_overseer_message.take() {
					Some(msg) => msg,
					None => overseer_recv(virtual_overseer).await,
				};

				if !matches!(
					&msg,
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(_hash, ..))
						if *_hash == hash
				) {
					// Ancestry has already been cached for this leaf.
					next_overseer_message.replace(msg);
					break
				}

				assert_matches!(
					msg,
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(.., tx)) => {
						let header = Header {
							parent_hash,
							number,
							state_root: Hash::zero(),
							extrinsics_root: Hash::zero(),
							digest: Default::default(),
						};

						tx.send(Ok(Some(header))).unwrap();
					}
				);

				requested_len += 1;
			}
		}

		for (hash, number) in ancestry_iter.take(requested_len) {
			let msg = match next_overseer_message.take() {
				Some(msg) => msg,
				None => virtual_overseer.recv().await,
			};
			assert_matches!(
				msg,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(parent, RuntimeApiRequest::Validators(tx))
				) if parent == hash => {
					tx.send(Ok(test_state.validator_public.clone())).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))
				) if parent == hash => {
					let validator_groups = test_state.validator_groups.clone();
					let mut group_rotation_info = test_state.group_rotation_info.clone();
					group_rotation_info.now = number;
					tx.send(Ok((validator_groups, group_rotation_info))).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(parent, RuntimeApiRequest::AvailabilityCores(tx))
				) if parent == hash => {
					tx.send(Ok(test_state.cores.clone())).unwrap();
				}
			);
		}
	}
}
