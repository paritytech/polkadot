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

/// Provides a backchannel to zombienet test runner.
use std::env;
use futures_util::{StreamExt, stream::SplitStream};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use parity_scale_codec as scale_codec;

#[macro_use]
extern crate lazy_static;

mod errors;
use errors::BackchannelError;

#[derive(Debug)]
pub struct ZombienetBackchannel {
    pub inner: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>
}

lazy_static! {
    static ref BACKCHANNEL_HOST: String = env::var("BACKCHANNEL_HOST").unwrap_or_else(|_| "backchannel".to_string());
    static ref BACKCHANNEL_PORT: String = env::var("BACKCHANNEL_PORT").unwrap_or_else(|_| "3000".to_string());
}


impl ZombienetBackchannel {
    pub async fn connect() -> Result<ZombienetBackchannel,BackchannelError> {
        let ws_url = format!("ws://{}:{}/ws", *BACKCHANNEL_HOST, *BACKCHANNEL_PORT);
        log::debug!("Connecting to : {}",&ws_url);
        let (ws_stream, _) = connect_async(ws_url).await.map_err(|_| BackchannelError::CantConnectToWS)?;
        let (_write, read) = ws_stream.split();

        Ok( ZombienetBackchannel {
            inner: read
        })
    }

    pub async fn send(key: &'static str, val: impl scale_codec::Encode) -> Result<(),BackchannelError> {
        let client = reqwest::Client::new();
        let encoded = val.encode();
        let body = String::from_utf8_lossy(&encoded);
        let zombienet_endpoint = format!("http://{}:{}/{}", *BACKCHANNEL_HOST, *BACKCHANNEL_PORT, key);

        log::debug!("Sending to : {}",&zombienet_endpoint);
        let _res = client.post(zombienet_endpoint)
            .body(body.to_string())
            .send()
            .await.map_err(|e| {
                log::error!("Error sending new item: {}",e);
                BackchannelError::SendItemFail
            })?;

        Ok(())
    }
}

