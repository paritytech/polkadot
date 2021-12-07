// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

//! Provides the possibility to coordination between malicious actors and
//! the zombienet test-runner, allowing to reference runtime's generated
//! values in the test specifications, through a bidirectional message passing
//! implemented as a `backchannel`.

use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use parity_scale_codec as codec;
use serde::{Deserialize, Serialize};
use std::env;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::{
	connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

mod errors;
use errors::BackchannelError;

#[derive(Debug)]
pub struct ZombienetBackchannel {
	broadcast_tx: broadcast::Sender<BackchannelItem>,
	ws_tx: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackchannelItem {
	key: String,
	value: String,
}

pub const ZOMBIENET: &str = "ZOMBIENET_BACKCHANNEL🧟🧟🧟";

impl ZombienetBackchannel {
	pub async fn init() -> Result<ZombienetBackchannel, BackchannelError> {
		let backchannel_host =
			env::var("BACKCHANNEL_HOST").unwrap_or_else(|_| "backchannel".to_string());
		let backchannel_port = env::var("BACKCHANNEL_PORT").unwrap_or_else(|_| "3000".to_string());
		let ws_url = format!("ws://{}:{}/ws", backchannel_host, backchannel_port);
		tracing::debug!(target = ZOMBIENET, "Connecting to : {}", &ws_url);
		let (ws_stream, _) =
			connect_async(ws_url).await.map_err(|_| BackchannelError::CantConnectToWS)?;
		let (write, mut read) = ws_stream.split();

		let (tx, _rx) = broadcast::channel(256);

		let tx1 = tx.clone();
		tokio::spawn(async move {
			while let Some(Ok(Message::Text(text))) = read.next().await {
				println!("es {}", &text);

				match serde_json::from_str::<BackchannelItem>(&text) {
					Ok(backchannel_item) => {
						if tx1.send(backchannel_item).is_err() {
							tracing::error!("Error sending through the channel");
							return;
						}
					}
					Err(_) => {
						tracing::error!("Invalid payload received");
					}
				}
			}
		});

		Ok(ZombienetBackchannel { broadcast_tx: tx, ws_tx: write })
	}

	pub fn subscribe(&self) -> broadcast::Receiver<BackchannelItem> {
		self.broadcast_tx.subscribe()
	}

	pub async fn send(
		&mut self,
		key: &'static str,
		val: impl codec::Encode,
	) -> Result<(), BackchannelError> {
		let encoded = val.encode();
		let backchannel_item = BackchannelItem {
			key: key.to_string(),
			value: String::from_utf8_lossy(&encoded).to_string(),
		};

		self.ws_tx
			.send(Message::Text(serde_json::to_string(&backchannel_item).unwrap()))
			.await
			.map_err(|e| {
				tracing::error!(target = ZOMBIENET, "Error sending new item: {}", e);
				BackchannelError::SendItemFail
			})?;

		Ok(())
	}
}
