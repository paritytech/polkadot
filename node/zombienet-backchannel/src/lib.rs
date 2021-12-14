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
use lazy_static::lazy_static;
use parity_scale_codec as codec;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Mutex;
use tokio::{net::TcpStream, sync::broadcast};
use tokio_tungstenite::{
	connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

mod errors;
use errors::BackchannelError;

lazy_static! {
	pub static ref ZOMBIENET_BACKCHANNEL: Mutex<Option<ZombienetBackchannel>> = Mutex::new(None);
}

#[derive(Debug)]
pub struct ZombienetBackchannel {
	broadcast_tx: broadcast::Sender<BackchannelItem>,
	ws_tx: broadcast::Sender<BackchannelItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackchannelItem {
	key: String,
	value: String,
}

pub struct Broadcaster;

pub const ZOMBIENET: &str = "🧟ZOMBIENET🧟";

impl Broadcaster {
	pub fn subscribe(&self) -> Result<broadcast::Receiver<BackchannelItem>, BackchannelError> {
		let mut zombienet_bkc = ZOMBIENET_BACKCHANNEL.lock().unwrap();
		let sender = zombienet_bkc.as_mut().unwrap().broadcast_tx.clone();
		if zombienet_bkc.is_some() {
			Ok(sender.subscribe())
		} else {
			Err(BackchannelError::Uninitialized)
		}
	}

	pub async fn send(
		&mut self,
		key: &'static str,
		val: impl codec::Encode,
	) -> Result<(), BackchannelError> {
		let mut zombienet_bkc = ZOMBIENET_BACKCHANNEL.lock().unwrap();
		if zombienet_bkc.is_none() {
			return Err(BackchannelError::Uninitialized);
		}
		let encoded = val.encode();
		let backchannel_item = BackchannelItem {
			key: key.to_string(),
			value: String::from_utf8_lossy(&encoded).to_string(),
		};

		let sender = zombienet_bkc.as_mut().unwrap().ws_tx.clone();
		sender.send(backchannel_item).map_err(|e| {
			tracing::error!(target = ZOMBIENET, "Error sending new item: {}", e);
			BackchannelError::SendItemFail
		})?;

		Ok(())
	}
}

impl ZombienetBackchannel {
	pub async fn init() -> Result<(), BackchannelError> {
		let mut zombienet_bkc = ZOMBIENET_BACKCHANNEL.lock().unwrap();
		if zombienet_bkc.is_none() {
			let backchannel_host =
				env::var("BACKCHANNEL_HOST").unwrap_or_else(|_| "backchannel".to_string());
			let backchannel_port =
				env::var("BACKCHANNEL_PORT").unwrap_or_else(|_| "3000".to_string());
			let ws_url = format!("ws://{}:{}/ws", backchannel_host, backchannel_port);
			tracing::debug!(target = ZOMBIENET, "Connecting to : {}", &ws_url);
			let (ws_stream, _) =
				connect_async(ws_url).await.map_err(|_| BackchannelError::CantConnectToWS)?;
			let (mut write, mut read) = ws_stream.split();

			let (tx, _rx) = broadcast::channel(256);
			let (tx_relay, mut rx_relay) = broadcast::channel::<BackchannelItem>(256);

			// receive from the ws and send to all subcribers
			let tx1 = tx.clone();
			tokio::spawn(async move {
				while let Some(Ok(Message::Text(text))) = read.next().await {
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

			// receive from subscribers and relay to ws
			tokio::spawn(async move {
				while let Ok(item) = rx_relay.recv().await {
					if write
						.send(Message::Text(serde_json::to_string(&item).unwrap()))
						.await
						.is_err()
					{
						tracing::error!("Error sending through ws");
					}
				}
			});

			*zombienet_bkc = Some(ZombienetBackchannel { broadcast_tx: tx, ws_tx: tx_relay });
			return Ok(());
		}

		Err(BackchannelError::AlreadyInitialized)
	}

	pub fn broadcaster() -> Result<Broadcaster, BackchannelError> {
		if ZOMBIENET_BACKCHANNEL.lock().unwrap().is_some() {
			Ok(Broadcaster {})
		} else {
			Err(BackchannelError::Uninitialized)
		}
	}
}
