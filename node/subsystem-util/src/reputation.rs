// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Utilites for peer reputation change message handling.
use polkadot_node_network_protocol::{PeerId, UnifiedReputationChange as Rep};

use futures::{channel::mpsc, prelude::*, StreamExt};

use polkadot_node_subsystem::{
	messages::{AllMessages, NetworkBridgeMessage},
	SubsystemSender,
};

use std::collections::HashMap;

/// The default interval at which the peer reportint task flushes all reputation changes
/// to the network bridge.
pub const REPUTATION_FLUSH_INTERVAL_MS: u64 = 3000;

const LOG_TARGET: &str = "parachain::peer-reporting-task";

/// Report a peer. The report is not immediately sent to the `network-bridge`, instead it will
/// be either merged to an existing previous report or buffered until the next flush timer tick.
pub async fn report_peer(peer: PeerId, rep: Rep, rep_sender: &mut mpsc::Sender<(PeerId, Rep)>) {
	match rep_sender.send((peer, rep)).await {
		Ok(_) => {},
		Err(err) => {
			gum::warn!(target: LOG_TARGET, ?err, "Failed to report peer");
		},
	}
}

/// This is a helper task which aggregates peer reports sent by `report_peer` and flushes them
/// every `flush_interval_ms` milliseconds to the `network-bridge`.
pub async fn peer_reporting_task(
	rep_receiver: mpsc::Receiver<(PeerId, Rep)>,
	mut sender: impl SubsystemSender,
	flush_interval_ms: Option<u64>,
) {
	let mut reports = HashMap::new();
	let mut message_type_distribution = HashMap::new();
	let mut received_report_count = 0;
	let from_subsystems = rep_receiver.fuse();
	let flush_interval_ms = flush_interval_ms.unwrap_or(REPUTATION_FLUSH_INTERVAL_MS);

	// We flush reputation changes to the network bridge on every tick.
	let interval = futures::stream::unfold((), move |_| async move {
		tokio::time::sleep(std::time::Duration::from_millis(flush_interval_ms)).await;
		Some(((), ()))
	})
	.fuse();

	futures::pin_mut!(from_subsystems, interval);

	loop {
		futures::select! {
			msg = from_subsystems.next() => {
				match msg {
					Some((peer_id, rep_change)) => {
						received_report_count += 1;

						// Record message type
						message_type_distribution.entry(rep_change.description()).and_modify(|count| {
							*count += 1;
						}).or_insert(1);

						reports.entry(peer_id).and_modify(|rep:&mut Rep| {
							rep.accumulate(rep_change);
						}).or_insert(rep_change);
					},
					None => panic!("peer reporting disconnected")
				}
			}
			_ = interval.next() => {
				gum::debug!(target: LOG_TARGET, %received_report_count, flush_count = %reports.len(), "Flushing rep changes");

				for (reason, count) in message_type_distribution.into_iter() {
					gum::debug!(target: LOG_TARGET, ?reason, %count, "Report reason distribution");

				}

				for (peer_id, rep) in reports.into_iter() {
					sender.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(peer_id, rep)))
						.await;
				}

				reports = HashMap::new();
				message_type_distribution = HashMap::new();

				received_report_count = 0;
			}
		}
	}
}
