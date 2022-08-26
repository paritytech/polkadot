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
	listener: TcpListener,
	broadcast_rx: BroadcastRx,
}

impl BridgeMirrorServer {
	pub async fn new(addr: String, broadcast_rx: BroadcastRx) -> Self {
		let try_socket = TcpListener::bind(&addr).await;
		let listener = try_socket.expect("Failed to bind");

		BridgeMirrorServer { peers: Default::default(), listener, broadcast_rx }
	}

	pub async fn run_once(&mut self) {
		select! {
			result = self.listener.accept().fuse() => {
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
					for peer in &self.peers {
						gum::info!(target: LOG_TARGET, "Sending message to peer");
						if let Err(err) = peer.unbounded_send(message.clone()) {
							gum::error!(target: LOG_TARGET, "Removing client: {:?}", err);
						}
					}
				}
			},
		};
	}
}

// Handle one client. Just forward whatever is received over the broadcast rx to the client.
async fn handle_one_client(mut broadcast_rx: BroadcastRx, raw_stream: TcpStream, addr: SocketAddr) {
	gum::info!(target: LOG_TARGET, "Incoming TCP connection from: {}", addr);

	let mut ws_stream = tokio_tungstenite::accept_async(raw_stream)
		.await
		.expect("Error during the websocket handshake occurred");
	gum::info!(target: LOG_TARGET, "WebSocket connection established: {}", addr);

	while let Some(message) = broadcast_rx.next().await {
		let ws_message = Message::Binary(message);
		if let Err(err) = ws_stream.send(ws_message).await {
			gum::error!(target: LOG_TARGET, "Error sending to {}: {}", addr, err);
			break
		}
	}
}
