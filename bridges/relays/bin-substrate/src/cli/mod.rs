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

use codec::{Decode, Encode};
use relay_substrate_client::ChainRuntimeVersion;
use sp_runtime::app_crypto::Ss58Codec;
use structopt::{clap::arg_enum, StructOpt};
use strum::{EnumString, EnumVariantNames};

use bp_messages::LaneId;

pub(crate) mod bridge;
pub(crate) mod encode_call;
pub(crate) mod encode_message;
pub(crate) mod estimate_fee;
pub(crate) mod send_message;

mod derive_account;
mod init_bridge;
mod register_parachain;
mod reinit_bridge;
mod relay_headers;
mod relay_headers_and_messages;
mod relay_messages;
mod resubmit_transactions;
mod swap_tokens;

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
	/// Reinitialize on-chain bridge pallet with current header data.
	///
	/// Sends all missing mandatory headers to bootstrap the bridge with current finalized block
	/// data.
	ReinitBridge(reinit_bridge::ReinitBridge),
	/// Send custom message over the bridge.
	///
	/// Allows interacting with the bridge by sending messages over `Messages` component.
	/// The message is being sent to the source chain, delivered to the target chain and dispatched
	/// there.
	SendMessage(send_message::SendMessage),
	/// Generate SCALE-encoded `Call` for the chosen network.
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
	/// Given a source chain `AccountId`, derive the corresponding `AccountId` for the target
	/// chain.
	DeriveAccount(derive_account::DeriveAccount),
	/// Resubmit transactions with increased tip if they are stalled.
	ResubmitTransactions(resubmit_transactions::ResubmitTransactions),
	/// Swap tokens using token-swap bridge.
	SwapTokens(swap_tokens::SwapTokens),
	/// Register parachain.
	RegisterParachain(register_parachain::RegisterParachain),
}

impl Command {
	// Initialize logger depending on the command.
	fn init_logger(&self) {
		use relay_utils::initialize::{initialize_logger, initialize_relay};

		match self {
			Self::RelayHeaders(_) |
			Self::RelayMessages(_) |
			Self::RelayHeadersAndMessages(_) |
			Self::InitBridge(_) => {
				initialize_relay();
			},
			_ => {
				initialize_logger(false);
			},
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
			Self::ReinitBridge(arg) => arg.run().await?,
			Self::SendMessage(arg) => arg.run().await?,
			Self::EncodeCall(arg) => arg.run().await?,
			Self::EncodeMessage(arg) => arg.run().await?,
			Self::EstimateFee(arg) => arg.run().await?,
			Self::DeriveAccount(arg) => arg.run().await?,
			Self::ResubmitTransactions(arg) => arg.run().await?,
			Self::SwapTokens(arg) => arg.run().await?,
			Self::RegisterParachain(arg) => arg.run().await?,
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
		Self { account, ss58_format: T::ss58_format().try_into().expect(SS58_FORMAT_PROOF) }
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
	/// Current version of the chain runtime, known to relay.
	const RUNTIME_VERSION: sp_version::RuntimeVersion;

	/// Crypto KeyPair type used to send messages.
	///
	/// In case of chains supporting multiple cryptos, pick one used by the CLI.
	type KeyPair: sp_core::crypto::Pair;

	/// Bridge Message Payload type.
	///
	/// TODO [#854] This should be removed in favor of target-specifc types.
	type MessagePayload;

	/// Numeric value of SS58 format.
	fn ss58_format() -> u16;

	/// Construct message payload to be sent over the bridge.
	fn encode_message(
		message: crate::cli::encode_message::MessagePayload,
	) -> anyhow::Result<Self::MessagePayload>;
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
			return Ok(ExplicitOrMaximal::Maximal)
		}

		V::from_str(s)
			.map(ExplicitOrMaximal::Explicit)
			.map_err(|e| format!("Failed to parse '{:?}'. Expected 'max' or explicit value", e))
	}
}

#[doc = "Runtime version params."]
#[derive(StructOpt, Debug, PartialEq, Eq, Clone, Copy, EnumString, EnumVariantNames)]
pub enum RuntimeVersionType {
	/// Auto query version from chain
	Auto,
	/// Custom `spec_version` and `transaction_version`
	Custom,
	/// Read version from bundle dependencies directly.
	Bundle,
}

/// Create chain-specific set of configuration objects: connection parameters,
/// signing parameters and bridge initialization parameters.
#[macro_export]
macro_rules! declare_chain_options {
	($chain:ident, $chain_prefix:ident) => {
		paste::item! {
			#[doc = $chain " connection params."]
			#[derive(StructOpt, Debug, PartialEq, Eq, Clone)]
			pub struct [<$chain ConnectionParams>] {
				#[doc = "Connect to " $chain " node at given host."]
				#[structopt(long, default_value = "127.0.0.1")]
				pub [<$chain_prefix _host>]: String,
				#[doc = "Connect to " $chain " node websocket server at given port."]
				#[structopt(long, default_value = "9944")]
				pub [<$chain_prefix _port>]: u16,
				#[doc = "Use secure websocket connection."]
				#[structopt(long)]
				pub [<$chain_prefix _secure>]: bool,
				#[doc = "Custom runtime version"]
				#[structopt(flatten)]
				pub [<$chain_prefix _runtime_version>]: [<$chain RuntimeVersionParams>],
			}

			#[doc = $chain " runtime version params."]
			#[derive(StructOpt, Debug, PartialEq, Eq, Clone, Copy)]
			pub struct [<$chain RuntimeVersionParams>] {
				#[doc = "The type of runtime version for chain " $chain]
				#[structopt(long, default_value = "Bundle")]
				pub [<$chain_prefix _version_mode>]: RuntimeVersionType,
				#[doc = "The custom sepc_version for chain " $chain]
				#[structopt(long)]
				pub [<$chain_prefix _spec_version>]: Option<u32>,
				#[doc = "The custom transaction_version for chain " $chain]
				#[structopt(long)]
				pub [<$chain_prefix _transaction_version>]: Option<u32>,
			}

			#[doc = $chain " signing params."]
			#[derive(StructOpt, Debug, PartialEq, Eq, Clone)]
			pub struct [<$chain SigningParams>] {
				#[doc = "The SURI of secret key to use when transactions are submitted to the " $chain " node."]
				#[structopt(long)]
				pub [<$chain_prefix _signer>]: Option<String>,
				#[doc = "The password for the SURI of secret key to use when transactions are submitted to the " $chain " node."]
				#[structopt(long)]
				pub [<$chain_prefix _signer_password>]: Option<String>,

				#[doc = "Path to the file, that contains SURI of secret key to use when transactions are submitted to the " $chain " node. Can be overridden with " $chain_prefix "_signer option."]
				#[structopt(long)]
				pub [<$chain_prefix _signer_file>]: Option<std::path::PathBuf>,
				#[doc = "Path to the file, that password for the SURI of secret key to use when transactions are submitted to the " $chain " node. Can be overridden with " $chain_prefix "_signer_password option."]
				#[structopt(long)]
				pub [<$chain_prefix _signer_password_file>]: Option<std::path::PathBuf>,

				#[doc = "Transactions mortality period, in blocks. MUST be a power of two in [4; 65536] range. MAY NOT be larger than `BlockHashCount` parameter of the chain system module."]
				#[structopt(long)]
				pub [<$chain_prefix _transactions_mortality>]: Option<u32>,
			}

			#[doc = "Parameters required to sign transaction on behalf of owner of the messages pallet at " $chain "."]
			#[derive(StructOpt, Debug, PartialEq, Eq)]
			pub struct [<$chain MessagesPalletOwnerSigningParams>] {
				#[doc = "The SURI of secret key to use when transactions are submitted to the " $chain " node."]
				#[structopt(long)]
				pub [<$chain_prefix _messages_pallet_owner>]: Option<String>,
				#[doc = "The password for the SURI of secret key to use when transactions are submitted to the " $chain " node."]
				#[structopt(long)]
				pub [<$chain_prefix _messages_pallet_owner_password>]: Option<String>,
			}

			impl [<$chain SigningParams>] {
				/// Return transactions mortality.
				#[allow(dead_code)]
				pub fn transactions_mortality(&self) -> anyhow::Result<Option<u32>> {
					self.[<$chain_prefix _transactions_mortality>]
						.map(|transactions_mortality| {
							if !(4..=65536).contains(&transactions_mortality)
								|| !transactions_mortality.is_power_of_two()
							{
								Err(anyhow::format_err!(
									"Transactions mortality {} is not a power of two in a [4; 65536] range",
									transactions_mortality,
								))
							} else {
								Ok(transactions_mortality)
							}
						})
						.transpose()
				}

				/// Parse signing params into chain-specific KeyPair.
				#[allow(dead_code)]
				pub fn to_keypair<Chain: CliChain>(&self) -> anyhow::Result<Chain::KeyPair> {
					let suri = match (self.[<$chain_prefix _signer>].as_ref(), self.[<$chain_prefix _signer_file>].as_ref()) {
						(Some(suri), _) => suri.to_owned(),
						(None, Some(suri_file)) => std::fs::read_to_string(suri_file)
							.map_err(|err| anyhow::format_err!(
								"Failed to read SURI from file {:?}: {}",
								suri_file,
								err,
							))?,
						(None, None) => return Err(anyhow::format_err!(
							"One of options must be specified: '{}' or '{}'",
							stringify!([<$chain_prefix _signer>]),
							stringify!([<$chain_prefix _signer_file>]),
						)),
					};

					let suri_password = match (
						self.[<$chain_prefix _signer_password>].as_ref(),
						self.[<$chain_prefix _signer_password_file>].as_ref(),
					) {
						(Some(suri_password), _) => Some(suri_password.to_owned()),
						(None, Some(suri_password_file)) => std::fs::read_to_string(suri_password_file)
							.map(Some)
							.map_err(|err| anyhow::format_err!(
								"Failed to read SURI password from file {:?}: {}",
								suri_password_file,
								err,
							))?,
						_ => None,
					};

					use sp_core::crypto::Pair;

					Chain::KeyPair::from_string(
						&suri,
						suri_password.as_deref()
					).map_err(|e| anyhow::format_err!("{:?}", e))
				}
			}

			#[allow(dead_code)]
			impl [<$chain MessagesPalletOwnerSigningParams>] {
				/// Parse signing params into chain-specific KeyPair.
				pub fn to_keypair<Chain: CliChain>(&self) -> anyhow::Result<Option<Chain::KeyPair>> {
					use sp_core::crypto::Pair;

					let [<$chain_prefix _messages_pallet_owner>] = match self.[<$chain_prefix _messages_pallet_owner>] {
						Some(ref messages_pallet_owner) => messages_pallet_owner,
						None => return Ok(None),
					};
					Chain::KeyPair::from_string(
						[<$chain_prefix _messages_pallet_owner>],
						self.[<$chain_prefix _messages_pallet_owner_password>].as_deref()
					).map_err(|e| anyhow::format_err!("{:?}", e)).map(Some)
				}
			}

			impl [<$chain ConnectionParams>] {
				/// Returns `true` if version guard can be started.
				///
				/// There's no reason to run version guard when version mode is set to `Auto`. It can
				/// lead to relay shutdown when chain is upgraded, even though we have explicitly
				/// said that we don't want to shutdown.
				#[allow(dead_code)]
				pub fn can_start_version_guard(&self) -> bool {
					self.[<$chain_prefix _runtime_version>].[<$chain_prefix _version_mode>] != RuntimeVersionType::Auto
				}

				/// Convert connection params into Substrate client.
				pub async fn to_client<Chain: CliChain>(
					&self,
				) -> anyhow::Result<relay_substrate_client::Client<Chain>> {
					let chain_runtime_version = self
						.[<$chain_prefix _runtime_version>]
						.into_runtime_version(Some(Chain::RUNTIME_VERSION))?;
					Ok(relay_substrate_client::Client::new(relay_substrate_client::ConnectionParams {
						host: self.[<$chain_prefix _host>].clone(),
						port: self.[<$chain_prefix _port>],
						secure: self.[<$chain_prefix _secure>],
						chain_runtime_version,
					})
					.await
					)
				}

				/// Return selected `chain_spec` version.
				///
				/// This function only connects to the node if version mode is set to `Auto`.
				#[allow(dead_code)]
				pub async fn selected_chain_spec_version<Chain: CliChain>(
					&self,
				) -> anyhow::Result<u32> {
					let chain_runtime_version = self
						.[<$chain_prefix _runtime_version>]
						.into_runtime_version(Some(Chain::RUNTIME_VERSION))?;
					Ok(match chain_runtime_version {
						ChainRuntimeVersion::Auto => self
							.to_client::<Chain>()
							.await?
							.simple_runtime_version()
							.await?
							.0,
						ChainRuntimeVersion::Custom(spec_version, _) => spec_version,
					})
				}
			}

			impl [<$chain RuntimeVersionParams>] {
				/// Converts self into `ChainRuntimeVersion`.
				pub fn into_runtime_version(
					self,
					bundle_runtime_version: Option<sp_version::RuntimeVersion>,
				) -> anyhow::Result<ChainRuntimeVersion> {
					Ok(match self.[<$chain_prefix _version_mode>] {
						RuntimeVersionType::Auto => ChainRuntimeVersion::Auto,
						RuntimeVersionType::Custom => {
							let except_spec_version = self.[<$chain_prefix _spec_version>]
								.ok_or_else(|| anyhow::Error::msg(format!("The {}-spec-version is required when choose custom mode", stringify!($chain_prefix))))?;
							let except_transaction_version = self.[<$chain_prefix _transaction_version>]
								.ok_or_else(|| anyhow::Error::msg(format!("The {}-transaction-version is required when choose custom mode", stringify!($chain_prefix))))?;
							ChainRuntimeVersion::Custom(
								except_spec_version,
								except_transaction_version
							)
						},
						RuntimeVersionType::Bundle => match bundle_runtime_version {
							Some(runtime_version) => ChainRuntimeVersion::Custom(
								runtime_version.spec_version,
								runtime_version.transaction_version
							),
							None => ChainRuntimeVersion::Auto
						},
					})
				}
			}
		}
	};
}

declare_chain_options!(Source, source);
declare_chain_options!(Target, target);
declare_chain_options!(Relaychain, relaychain);
declare_chain_options!(Parachain, parachain);

#[cfg(test)]
mod tests {
	use std::str::FromStr;

	use sp_core::Pair;

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
		let parsed = expected.iter().map(|s| AccountId::from_str(s).unwrap()).collect::<Vec<_>>();

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

	#[test]
	fn reads_suri_from_file() {
		const ALICE: &str = "//Alice";
		const BOB: &str = "//Bob";
		const ALICE_PASSWORD: &str = "alice_password";
		const BOB_PASSWORD: &str = "bob_password";

		let alice = sp_core::sr25519::Pair::from_string(ALICE, Some(ALICE_PASSWORD)).unwrap();
		let bob = sp_core::sr25519::Pair::from_string(BOB, Some(BOB_PASSWORD)).unwrap();
		let bob_with_alice_password =
			sp_core::sr25519::Pair::from_string(BOB, Some(ALICE_PASSWORD)).unwrap();

		let temp_dir = tempfile::tempdir().unwrap();
		let mut suri_file_path = temp_dir.path().to_path_buf();
		let mut password_file_path = temp_dir.path().to_path_buf();
		suri_file_path.push("suri");
		password_file_path.push("password");
		std::fs::write(&suri_file_path, BOB.as_bytes()).unwrap();
		std::fs::write(&password_file_path, BOB_PASSWORD.as_bytes()).unwrap();

		// when both seed and password are read from file
		assert_eq!(
			TargetSigningParams {
				target_signer: Some(ALICE.into()),
				target_signer_password: Some(ALICE_PASSWORD.into()),

				target_signer_file: None,
				target_signer_password_file: None,

				target_transactions_mortality: None,
			}
			.to_keypair::<relay_rialto_client::Rialto>()
			.map(|p| p.public())
			.map_err(drop),
			Ok(alice.public()),
		);

		// when both seed and password are read from file
		assert_eq!(
			TargetSigningParams {
				target_signer: None,
				target_signer_password: None,

				target_signer_file: Some(suri_file_path.clone()),
				target_signer_password_file: Some(password_file_path.clone()),

				target_transactions_mortality: None,
			}
			.to_keypair::<relay_rialto_client::Rialto>()
			.map(|p| p.public())
			.map_err(drop),
			Ok(bob.public()),
		);

		// when password are is overriden by cli option
		assert_eq!(
			TargetSigningParams {
				target_signer: None,
				target_signer_password: Some(ALICE_PASSWORD.into()),

				target_signer_file: Some(suri_file_path.clone()),
				target_signer_password_file: Some(password_file_path.clone()),

				target_transactions_mortality: None,
			}
			.to_keypair::<relay_rialto_client::Rialto>()
			.map(|p| p.public())
			.map_err(drop),
			Ok(bob_with_alice_password.public()),
		);

		// when both seed and password are overriden by cli options
		assert_eq!(
			TargetSigningParams {
				target_signer: Some(ALICE.into()),
				target_signer_password: Some(ALICE_PASSWORD.into()),

				target_signer_file: Some(suri_file_path),
				target_signer_password_file: Some(password_file_path),

				target_transactions_mortality: None,
			}
			.to_keypair::<relay_rialto_client::Rialto>()
			.map(|p| p.public())
			.map_err(drop),
			Ok(alice.public()),
		);
	}
}
