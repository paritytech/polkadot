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

//! Traffic mirroring facitilites for Network Bridge Subsystem TX-side.
use std::net::SocketAddr;

use futures::{
	channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender},
	select, FutureExt, SinkExt,
};
use futures_util::StreamExt;

use tokio::net::{TcpListener, TcpStream};
use tungstenite::protocol::Message;

const LOG_TARGET: &'static str = "parachain::network-bridge-tx-mirror";

/// Used to receive bridge broadcast messages.
pub type BroadcastRx = UnboundedReceiver<Vec<u8>>;
pub type BroadcastTx = UnboundedSender<Vec<u8>>;

pub struct BridgeMirrorServer {
	peers: Vec<BroadcastTx>,
	listener: Option<TcpListener>,
	broadcast_rx: BroadcastRx,
	addr: String,
}

impl BridgeMirrorServer {
	pub fn new(addr: String, broadcast_rx: BroadcastRx) -> Self {

		BridgeMirrorServer {
			peers: Default::default(),
			listener: None,
			broadcast_rx,
			addr,
		}
	}

	/// Start the websocket server - will try to find port to listen on.
	pub async fn start(&mut self) {
		let mut listener = None;
		for i in 0..100 {
			let addr = format!("{}:{}", self.addr, 13370 + i);
			gum::info!("Trying to setup websocket server at `{}`", addr);
			if let Ok(maybe_listener) = TcpListener::bind(&addr).await {
				listener = Some(maybe_listener);
				gum::info!("Listening on `{}`", addr);
				break
			}
		}
		self.listener = Some(listener.expect("Giving up, unable to find spare port."));
	}

	/// Poll exactly one message or accept a connection over the websocket. 
	pub async fn run_once(&mut self) {
		if let Some(listener) = self.listener.as_ref() {
			select! {
				result = listener.accept().fuse() => {
					if let Ok((stream, addr)) = result {
						let (broadcast_tx, broadcast_rx) = unbounded();
						self.peers.push(broadcast_tx);
						tokio::spawn(handle_one_client(broadcast_rx, stream, addr));
					} else {
						gum::error!(target: LOG_TARGET, "Error listening: {:?}", result);
					}
				},
				message = self.broadcast_rx.next().fuse() => {
					if let Some(message) = message {
						let mut peers_to_remove = Vec::new();
						for (idx, peer) in self.peers.iter().enumerate() {
							gum::info!(target: LOG_TARGET, "Sending message to peer");
							if let Err(err) = peer.unbounded_send(message.clone()) {
								gum::error!(target: LOG_TARGET, "Removing client: {:?}", err);
								peers_to_remove.push(idx);
							}
						}
	
						for peer in peers_to_remove {
							self.peers.remove(peer);
						}
					}
				},
			};
		}
	}
}

// Handle one client. Just forward whatever is received over the broadcast rx to the client.
async fn handle_one_client(mut broadcast_rx: BroadcastRx, raw_stream: TcpStream, addr: SocketAddr) {
	gum::info!(target: LOG_TARGET, "Incoming TCP connection from: {}", addr);

	let mut ws_stream = match tokio_tungstenite::accept_async(raw_stream).await {
		Ok(ws_stream) => ws_stream,
		Err(err) => {
			gum::warn!(
				target: LOG_TARGET,
				"Incoming bridge mirroring webSocket connection failed: {}",
				err
			);
			return
		},
	};

	gum::info!(target: LOG_TARGET, "WebSocket connection established: {}", addr);

	while let Some(message) = broadcast_rx.next().await {
		let ws_message = Message::Binary(message);
		if let Err(err) = ws_stream.send(ws_message).await {
			gum::error!(target: LOG_TARGET, "Error sending to {}: {}", addr, err);
			break
		}
	}
}
