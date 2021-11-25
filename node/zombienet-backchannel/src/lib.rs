/// Provides a backchannel to zombienet test runner.

use std::env;

use futures_util::{StreamExt, stream::SplitStream};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use parity_scale_codec as scale_codec;

mod errors;
use errors::BackchannelError;

#[derive(Debug)]
pub struct ZombienetBackchannel {
    pub inner: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>
}

const BACKCHANNEL_HOST: &str = "backchannel";
const BACKCHANNEL_PORT: &str = "3000";

impl ZombienetBackchannel {
    pub async fn connect() -> Result<ZombienetBackchannel,BackchannelError> {
        let backchannel_host = env::var("BACKCHANNEL_HOST").unwrap_or_else(|_| BACKCHANNEL_HOST.to_string());
        let backchannel_port = env::var("BACKCHANNEL_PORT").unwrap_or_else(|_| BACKCHANNEL_PORT.to_string());

        let ws_url = format!("ws://{}:{}/ws", backchannel_host, backchannel_port);
        log::debug!("Connecting to : {}",&ws_url);
        let (ws_stream, _) = connect_async(ws_url).await.map_err(|_| BackchannelError::CantConnectToWS)?;
        let (_write, read) = ws_stream.split();

        Ok( ZombienetBackchannel {
            inner: read
        })
    }

    pub async fn send(key: &'static str, val: impl scale_codec::Encode) -> Result<(),BackchannelError> {
        let backchannel_host = env::var("BACKCHANNEL_HOST").unwrap_or_else(|_| BACKCHANNEL_HOST.to_string());
        let backchannel_port = env::var("BACKCHANNEL_PORT").unwrap_or_else(|_| BACKCHANNEL_PORT.to_string());

        let client = reqwest::Client::new();
        let encoded = val.encode();
        let body = String::from_utf8_lossy(&encoded);
        let zombienet_endpoint = format!("http://{}:{}/{}",backchannel_host, backchannel_port, key);

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

