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

/// Provides the possibility to coordination between malicious actors and 
/// the zombienet test-runner, allowing to reference runtime's generated
/// values in the test specifications, through a bidirectional message passing
/// implemented as a `backchannel`.
use std::env;
use futures_util::{StreamExt, stream::SplitStream};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use parity_scale_codec as codec;
use lazy_static::lazy_static;

mod errors;
use errors::BackchannelError;

#[derive(Debug)]
pub struct ZombienetBackchannel {
    pub inner: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>
}

pub const ZOMBIENET: &str = "ZOMBIENET_BACKCHANNEL🧟🧟🧟";

lazy_static! {
    static ref BACKCHANNEL_HOST: String = env::var("BACKCHANNEL_HOST").unwrap_or_else(|_| "backchannel".to_string());
    static ref BACKCHANNEL_PORT: String = env::var("BACKCHANNEL_PORT").unwrap_or_else(|_| "3000".to_string());
}


impl ZombienetBackchannel {
    pub async fn connect() -> Result<ZombienetBackchannel,BackchannelError> {
        let ws_url = format!("ws://{}:{}/ws", *BACKCHANNEL_HOST, *BACKCHANNEL_PORT);
        tracing::debug!(target = ZOMBIENET, "Connecting to : {}", &ws_url);
        let (ws_stream, _) = connect_async(ws_url).await.map_err(|_| BackchannelError::CantConnectToWS)?;
        let (_write, read) = ws_stream.split();

        Ok( ZombienetBackchannel {
            inner: read
        })
    }

    pub async fn send(key: &'static str, val: impl codec::Encode) -> Result<(),BackchannelError> {
        let client = reqwest::Client::new();
        let encoded = val.encode();
        let body = String::from_utf8_lossy(&encoded);
        let zombienet_endpoint = format!("http://{}:{}/{}", *BACKCHANNEL_HOST, *BACKCHANNEL_PORT, key);

        tracing::debug!(target = ZOMBIENET, "Sending to: {}",&zombienet_endpoint);
        let _res = client.post(zombienet_endpoint)
            .body(body.to_string())
            .send()
            .await.map_err(|e| {
                tracing::error!(target = ZOMBIENET, "Error sending new item: {}",e);
                BackchannelError::SendItemFail
            })?;

        Ok(())
    }
}

