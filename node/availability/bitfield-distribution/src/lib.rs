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

//! The bitfield distribution subsystem spreading @todo .

use bridge::NetworkBridgeMessage;
use futures::{channel::oneshot, Future};
use node_primitives::{ProtocolId, SignedFullStatement, View};
use polkadot_node_subsystem::{
    messages::{AllMessages, BitfieldDistributionMessage},
    OverseerSignal, SubsystemResult,
};
use polkadot_node_subsystem::{FromOverseer, SpawnedSubsystem, Subsystem, SubsystemContext};
use polkadot_primitives::Hash;
use std::{collections::HashMap, pin::Pin};

// @todo split in multiple costs
const COST_UNEXPECTED: Rep = Rep::new(-100, "Unexpected");

#[derive(Default, Clone)]
struct Tracker {
	// track all active peers and their views
	// to determine what is relevant to them
    peer_views: HashMap<PeerId, View>,

    // our current view
    view: View,
}



fn network_update_message(n: NetworkBridgeEvent) -> AllMessages {
	AllMessages::BitfieldDistribution(BitfieldDistributionMessage::NetworkBridgeUpdate(n))
}

pub struct BitfieldDistribution;

impl BitfieldDistribution {
    const PROTOCOL_ID: ProtocolId = *b"bitd";

    async fn run<Context>(mut ctx: Context) -> SubsystemResult<()>
    where
        Context: SubsystemContext<Message = BitfieldSigningMessage>,
    {
		// startup: register the network protocol with the bridge.
		ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::RegisterEventProducer(
			Self::PROTOCOL_ID,
			handle_network_msg,
		))).await?;

		let mut data = Tracker::default();
		loop {
			{
				let x = ctx.recv().await?;
				match x {
					FromOverseer::Communication { msg: _ } => {
						unreachable!("BitfieldDistributionMessage does not exist; qed")
					}
					FromOverseer::Signal(OverseerSignal::StartWork(hash)) => {
						// @todo cannot work
						// tracker.active_heads.insert(hash.clone(), process(&mut data, hash));
					}
					FromOverseer::Signal(OverseerSignal::StopWork(hash)) => {
						// could work, but see above
						tracker.active_heads.remove(&hash);
					}
					FromOverseer::Signal(OverseerSignal::Conclude) => break,
				}
			}
			active_jobs.retain(|_, future| future.poll().is_pending());
		}
		Ok(())
    }
}

/// Handle an incoming message
async fn process_incoming(
    tracker: &mut Tracker,
    message: BitfieldDistributionMessage,
) -> SubsystemResult<()> {
    match message {
        /// Distribute a bitfield via gossip to other validators.
        BitfieldDistributionMessage::DistributeBitfield(hash, signed_availability) => {
			// @todo check signature, where to get the SingingContext<Hash> from?
			// signed_availability.check_signature(signing_ctx, validator_id)?;

			for (peerid, view) in tracker.peer_views.filter(|(_peerid,view)| {
				view.contains(hash)
			}) {
				// @todo verify sequential execution is ok or if spawning tasks is better


			}
		}
        BitfieldDistributionMessage::NetworkBridgeUpdate(event) => {
            handle_network_msg(
                &mut tracker,
                &mut ctx,
                event,
            )
            .await?
        }
	}
	Ok(())
}

/// Deal with network bridge updates and track what needs to be tracked
async fn handle_network_msg(
	mut ctx: impl SubsystemContext<Message = BitfieldDistributionMessage>,
    tracker: &mut Tracker,
    bridge_event: NetworkBridgeMessage,
) -> SubsystemResult<()> {
    match bridge_message {
        NetworkBridgeMessage::PeerConnected(peerid, _role) => {
			// insert if none already present
			tracker.peer_views.entry(peerid).or_insert(View::default());
        }
        NetworkBridgeMessage::PeerDisconnected(peerid) => {
			// get rid of superfluous data
			tracker.peer_views.remove(peerid);
		}
        NetworkBridgeMessage::PeerViewChange(peerid, view) => {
			tracker.peer_views.entry(peerid).modify(|val| {
				*val = view
			});

		},
        NetworkBridgeEvent::OurViewChange(view) => {
            let old_view = std::mem::replace(tracker.view, view);
            tracker
                .active_heads
                .retain(|head, _| tracker.view.contains(head));

            for new in tracker.view.difference(&old_view) {
                if !tracker.active_heads.contains_key(&new) {
					log::warn!("Active head running that's not active anymore, go catch it") //@todo rephrase
					//@todo should we get rid of that right here
				}
            }
        }
	}
	Ok(())
}

impl<C> Subsystem<C> for BitfieldDistribution
where
    C: SubsystemContext<Message = BitfieldDistributionMessage>,
{
    fn start(self, ctx: C) -> SpawnedSubsystem {
        SpawnedSubsystem(Box::pin(async move {
            Self::run(ctx).await;
        }))
    }
}
