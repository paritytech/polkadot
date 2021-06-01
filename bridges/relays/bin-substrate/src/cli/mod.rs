// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Deal with CLI args of substrate-to-substrate relay.

use std::convert::TryInto;

use bp_messages::LaneId;
use codec::{Decode, Encode};
use frame_support::weights::Weight;
use sp_runtime::app_crypto::Ss58Codec;
use structopt::{clap::arg_enum, StructOpt};

pub(crate) mod bridge;
pub(crate) mod encode_call;
pub(crate) mod encode_message;
pub(crate) mod estimate_fee;
pub(crate) mod send_message;

mod derive_account;
mod init_bridge;
mod relay_headers;
mod relay_headers_and_messages;
mod relay_messages;

/// Parse relay CLI args.
pub fn parse_args() -> Command {
	Command::from_args()
}

/// Substrate-to-Substrate bridge utilities.
#[derive(StructOpt)]
#[structopt(about = "Substrate-to-Substrate relay")]
pub enum Command {
	/// Start headers relay between two chains.
	///
	/// The on-chain bridge component should have been already initialized with
	/// `init-bridge` sub-command.
	RelayHeaders(relay_headers::RelayHeaders),
	/// Start messages relay between two chains.
	///
	/// Ties up to `Messages` pallets on both chains and starts relaying messages.
	/// Requires the header relay to be already running.
	RelayMessages(relay_messages::RelayMessages),
	/// Start headers and messages relay between two Substrate chains.
	///
	/// This high-level relay internally starts four low-level relays: two `RelayHeaders`
	/// and two `RelayMessages` relays. Headers are only relayed when they are required by
	/// the message relays - i.e. when there are messages or confirmations that needs to be
	/// relayed between chains.
	RelayHeadersAndMessages(relay_headers_and_messages::RelayHeadersAndMessages),
	/// Initialize on-chain bridge pallet with current header data.
	///
	/// Sends initialization transaction to bootstrap the bridge with current finalized block data.
	InitBridge(init_bridge::InitBridge),
	/// Send custom message over the bridge.
	///
	/// Allows interacting with the bridge by sending messages over `Messages` component.
	/// The message is being sent to the source chain, delivered to the target chain and dispatched
	/// there.
	SendMessage(send_message::SendMessage),
	/// Generate SCALE-encoded `Call` for choosen network.
	///
	/// The call can be used either as message payload or can be wrapped into a transaction
	/// and executed on the chain directly.
	EncodeCall(encode_call::EncodeCall),
	/// Generate SCALE-encoded `MessagePayload` object that can be sent over selected bridge.
	///
	/// The `MessagePayload` can be then fed to `Messages::send_message` function and sent over
	/// the bridge.
	EncodeMessage(encode_message::EncodeMessage),
	/// Estimate Delivery and Dispatch Fee required for message submission to messages pallet.
	EstimateFee(estimate_fee::EstimateFee),
	/// Given a source chain `AccountId`, derive the corresponding `AccountId` for the target chain.
	DeriveAccount(derive_account::DeriveAccount),
}

impl Command {
	// Initialize logger depending on the command.
	fn init_logger(&self) {
		use relay_utils::initialize::{initialize_logger, initialize_relay};

		match self {
			Self::RelayHeaders(_) | Self::RelayMessages(_) | Self::RelayHeadersAndMessages(_) | Self::InitBridge(_) => {
				initialize_relay();
			}
			_ => {
				initialize_logger(false);
			}
		}
	}

	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		self.init_logger();
		match self {
			Self::RelayHeaders(arg) => arg.run().await?,
			Self::RelayMessages(arg) => arg.run().await?,
			Self::RelayHeadersAndMessages(arg) => arg.run().await?,
			Self::InitBridge(arg) => arg.run().await?,
			Self::SendMessage(arg) => arg.run().await?,
			Self::EncodeCall(arg) => arg.run().await?,
			Self::EncodeMessage(arg) => arg.run().await?,
			Self::EstimateFee(arg) => arg.run().await?,
			Self::DeriveAccount(arg) => arg.run().await?,
		}
		Ok(())
	}
}

arg_enum! {
	#[derive(Debug)]
	/// The origin to use when dispatching the message on the target chain.
	///
	/// - `Target` uses account existing on the target chain (requires target private key).
	/// - `Origin` uses account derived from the source-chain account.
	pub enum Origins {
		Target,
		Source,
	}
}

/// Generic balance type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Balance(pub u128);

impl std::fmt::Display for Balance {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		use num_format::{Locale, ToFormattedString};
		write!(fmt, "{}", self.0.to_formatted_string(&Locale::en))
	}
}

impl std::str::FromStr for Balance {
	type Err = <u128 as std::str::FromStr>::Err;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(Self(s.parse()?))
	}
}

impl Balance {
	/// Cast balance to `u64` type, panicking if it's too large.
	pub fn cast(&self) -> u64 {
		self.0.try_into().expect("Balance is too high for this chain.")
	}
}

/// Generic account id with custom parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountId {
	account: sp_runtime::AccountId32,
	ss58_format: sp_core::crypto::Ss58AddressFormat,
}

impl std::fmt::Display for AccountId {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(fmt, "{}", self.account.to_ss58check_with_version(self.ss58_format))
	}
}

impl std::str::FromStr for AccountId {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let (account, ss58_format) = sp_runtime::AccountId32::from_ss58check_with_version(s)
			.map_err(|err| format!("Unable to decode SS58 address: {:?}", err))?;
		Ok(Self { account, ss58_format })
	}
}

const SS58_FORMAT_PROOF: &str = "u16 -> Ss58Format is infallible; qed";

impl AccountId {
	/// Create new SS58-formatted address from raw account id.
	pub fn from_raw<T: CliChain>(account: sp_runtime::AccountId32) -> Self {
		Self {
			account,
			ss58_format: T::ss58_format().try_into().expect(SS58_FORMAT_PROOF),
		}
	}

	/// Enforces formatting account to be for given [`CliChain`] type.
	///
	/// This will change the `ss58format` of the account to match the requested one.
	/// Note that a warning will be produced in case the current format does not match
	/// the requested one, but the conversion always succeeds.
	pub fn enforce_chain<T: CliChain>(&mut self) {
		let original = self.clone();
		self.ss58_format = T::ss58_format().try_into().expect(SS58_FORMAT_PROOF);
		log::debug!("{} SS58 format: {} (RAW: {})", self, self.ss58_format, self.account);
		if original.ss58_format != self.ss58_format {
			log::warn!(
				target: "bridge",
				"Address {} does not seem to match {}'s SS58 format (got: {}, expected: {}).\nConverted to: {}",
				original,
				T::NAME,
				original.ss58_format,
				self.ss58_format,
				self,
			)
		}
	}

	/// Returns the raw (no SS58-prefixed) account id.
	pub fn raw_id(&self) -> sp_runtime::AccountId32 {
		self.account.clone()
	}
}

/// Bridge-supported network definition.
///
/// Used to abstract away CLI commands.
pub trait CliChain: relay_substrate_client::Chain {
	/// Chain's current version of the runtime.
	const RUNTIME_VERSION: sp_version::RuntimeVersion;

	/// Crypto keypair type used to send messages.
	///
	/// In case of chains supporting multiple cryptos, pick one used by the CLI.
	type KeyPair: sp_core::crypto::Pair;

	/// Bridge Message Payload type.
	///
	/// TODO [#854] This should be removed in favour of target-specifc types.
	type MessagePayload;

	/// Numeric value of SS58 format.
	fn ss58_format() -> u16;

	/// Construct message payload to be sent over the bridge.
	fn encode_message(message: crate::cli::encode_message::MessagePayload) -> Result<Self::MessagePayload, String>;

	/// Maximal extrinsic weight (from the runtime).
	fn max_extrinsic_weight() -> Weight;
}

/// Lane id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexLaneId(pub LaneId);

impl From<HexLaneId> for LaneId {
	fn from(lane_id: HexLaneId) -> LaneId {
		lane_id.0
	}
}

impl std::str::FromStr for HexLaneId {
	type Err = hex::FromHexError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let mut lane_id = LaneId::default();
		hex::decode_to_slice(s, &mut lane_id)?;
		Ok(HexLaneId(lane_id))
	}
}

/// Nicer formatting for raw bytes vectors.
#[derive(Default, Encode, Decode, PartialEq, Eq)]
pub struct HexBytes(pub Vec<u8>);

impl std::str::FromStr for HexBytes {
	type Err = hex::FromHexError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(Self(hex::decode(s)?))
	}
}

impl std::fmt::Debug for HexBytes {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(fmt, "0x{}", self)
	}
}

impl std::fmt::Display for HexBytes {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(fmt, "{}", hex::encode(&self.0))
	}
}

impl HexBytes {
	/// Encode given object and wrap into nicely formatted bytes.
	pub fn encode<T: Encode>(t: &T) -> Self {
		Self(t.encode())
	}
}

/// Prometheus metrics params.
#[derive(StructOpt)]
pub struct PrometheusParams {
	/// Do not expose a Prometheus metric endpoint.
	#[structopt(long)]
	pub no_prometheus: bool,
	/// Expose Prometheus endpoint at given interface.
	#[structopt(long, default_value = "127.0.0.1")]
	pub prometheus_host: String,
	/// Expose Prometheus endpoint at given port.
	#[structopt(long, default_value = "9616")]
	pub prometheus_port: u16,
}

impl From<PrometheusParams> for relay_utils::metrics::MetricsParams {
	fn from(cli_params: PrometheusParams) -> relay_utils::metrics::MetricsParams {
		if !cli_params.no_prometheus {
			Some(relay_utils::metrics::MetricsAddress {
				host: cli_params.prometheus_host,
				port: cli_params.prometheus_port,
			})
			.into()
		} else {
			None.into()
		}
	}
}

/// Either explicit or maximal allowed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplicitOrMaximal<V> {
	/// User has explicitly specified argument value.
	Explicit(V),
	/// Maximal allowed value for this argument.
	Maximal,
}

impl<V: std::str::FromStr> std::str::FromStr for ExplicitOrMaximal<V>
where
	V::Err: std::fmt::Debug,
{
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		if s.to_lowercase() == "max" {
			return Ok(ExplicitOrMaximal::Maximal);
		}

		V::from_str(s)
			.map(ExplicitOrMaximal::Explicit)
			.map_err(|e| format!("Failed to parse '{:?}'. Expected 'max' or explicit value", e))
	}
}

/// Create chain-specific set of configuration objects: connection parameters,
/// signing parameters and bridge initialisation parameters.
#[macro_export]
macro_rules! declare_chain_options {
	($chain:ident, $chain_prefix:ident) => {
		paste::item! {
			#[doc = $chain " connection params."]
			#[derive(StructOpt, Debug, PartialEq, Eq)]
			pub struct [<$chain ConnectionParams>] {
				#[doc = "Connect to " $chain " node at given host."]
				#[structopt(long, default_value = "127.0.0.1")]
				pub [<$chain_prefix _host>]: String,
				#[doc = "Connect to " $chain " node websocket server at given port."]
				#[structopt(long)]
				pub [<$chain_prefix _port>]: u16,
				#[doc = "Use secure websocket connection."]
				#[structopt(long)]
				pub [<$chain_prefix _secure>]: bool,
			}

			#[doc = $chain " signing params."]
			#[derive(StructOpt, Debug, PartialEq, Eq)]
			pub struct [<$chain SigningParams>] {
				#[doc = "The SURI of secret key to use when transactions are submitted to the " $chain " node."]
				#[structopt(long)]
				pub [<$chain_prefix _signer>]: String,
				#[doc = "The password for the SURI of secret key to use when transactions are submitted to the " $chain " node."]
				#[structopt(long)]
				pub [<$chain_prefix _signer_password>]: Option<String>,
			}

			impl [<$chain SigningParams>] {
				/// Parse signing params into chain-specific KeyPair.
				pub fn to_keypair<Chain: CliChain>(&self) -> anyhow::Result<Chain::KeyPair> {
					use sp_core::crypto::Pair;

					Chain::KeyPair::from_string(
						&self.[<$chain_prefix _signer>],
						self.[<$chain_prefix _signer_password>].as_deref()
					).map_err(|e| anyhow::format_err!("{:?}", e))
				}
			}

			impl [<$chain ConnectionParams>] {
				/// Convert connection params into Substrate client.
				pub async fn to_client<Chain: CliChain>(
					&self,
				) -> anyhow::Result<relay_substrate_client::Client<Chain>> {
					Ok(relay_substrate_client::Client::new(relay_substrate_client::ConnectionParams {
						host: self.[<$chain_prefix _host>].clone(),
						port: self.[<$chain_prefix _port>],
						secure: self.[<$chain_prefix _secure>],
					})
					.await?
					)
				}
			}
		}
	};
}

declare_chain_options!(Source, source);
declare_chain_options!(Target, target);

#[cfg(test)]
mod tests {
	use std::str::FromStr;

	use super::*;

	#[test]
	fn should_format_addresses_with_ss58_format() {
		// given
		let rialto1 = "5sauUXUfPjmwxSgmb3tZ5d6yx24eZX4wWJ2JtVUBaQqFbvEU";
		let rialto2 = "5rERgaT1Z8nM3et2epA5i1VtEBfp5wkhwHtVE8HK7BRbjAH2";
		let millau1 = "752paRyW1EGfq9YLTSSqcSJ5hqnBDidBmaftGhBo8fy6ypW9";
		let millau2 = "74GNQjmkcfstRftSQPJgMREchqHM56EvAUXRc266cZ1NYVW5";

		let expected = vec![rialto1, rialto2, millau1, millau2];

		// when
		let parsed = expected
			.iter()
			.map(|s| AccountId::from_str(s).unwrap())
			.collect::<Vec<_>>();

		let actual = parsed.iter().map(|a| format!("{}", a)).collect::<Vec<_>>();

		assert_eq!(actual, expected)
	}

	#[test]
	fn hex_bytes_display_matches_from_str_for_clap() {
		// given
		let hex = HexBytes(vec![1, 2, 3, 4]);
		let display = format!("{}", hex);

		// when
		let hex2: HexBytes = display.parse().unwrap();

		// then
		assert_eq!(hex.0, hex2.0);
	}
}
