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


use log::{trace, warn};
use polkadot_primitives::v1::{
	Id as ParaId,
};
use super::{TARGET, Error, Result};
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, Subsystem, SubsystemContext, SubsystemError, SpawnedSubsystem,
	messages::{
		AllMessages, CollatorProtocolMessage,
	},
};
use polkadot_node_network_protocol::{
	v1 as protocol_v1, View,
};

#[derive(Default)]
struct State {
	/// The para this collator is collating on.
	collating_on: Option<ParaId>,

	/// Our own view.
	view: View,
}

async fn process_msg(
	msg: CollatorProtocolMessage,
	state: &mut State,
) -> Result<()> {
	use CollatorProtocolMessage::*;

	match msg {
		CollateOn(id) => {
			match state.collating_on {
				None => state.collating_on = Some(id),
				Some(our_id) => {
					trace!(
						target: TARGET,
						"Cannot collate on {}, already collating on {}", id, our_id,
					);
				}
			}
		}
		DistributeCollation(receipt, pov) => {
		},
		FetchCollation(_, _, _) => {
		},
		ReportCollator(_) => {
		},
		NoteGoodCollation(_) => {
		},
		NetworkBridgeUpdateV1(_) => {
		},
	}

	Ok(())
}

pub(crate) async fn run<Context>(mut ctx: Context) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use FromOverseer::*;
	use OverseerSignal::*;

	let mut state = State::default();

	loop {
		match ctx.recv().await? {
			Communication { msg } => process_msg(msg, &mut state).await?,
			Signal(ActiveLeaves(_update)) => {}
			Signal(BlockFinalized(_)) => {}
			Signal(Conclude) => break,
		}
	}

	Ok(())
}
