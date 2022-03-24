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

use crate::cli::{
	swap_tokens::wait_until_transaction_is_finalized, Balance, ParachainConnectionParams,
	RelaychainConnectionParams, RelaychainSigningParams,
};

use codec::Encode;
use frame_support::Twox64Concat;
use num_traits::Zero;
use polkadot_parachain::primitives::{
	HeadData as ParaHeadData, Id as ParaId, ValidationCode as ParaValidationCode,
};
use polkadot_runtime_common::{
	paras_registrar::Call as ParaRegistrarCall, slots::Call as ParaSlotsCall,
};
use polkadot_runtime_parachains::paras::ParaLifecycle;
use relay_substrate_client::{
	AccountIdOf, CallOf, Chain, Client, SignParam, TransactionSignScheme, UnsignedTransaction,
};
use rialto_runtime::SudoCall;
use sp_core::{
	storage::{well_known_keys::CODE, StorageKey},
	Bytes, Pair,
};
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames};

/// Name of the `NextFreeParaId` value in the `polkadot_runtime_common::paras_registrar` pallet.
const NEXT_FREE_PARA_ID_STORAGE_NAME: &str = "NextFreeParaId";
/// Name of the `ParaLifecycles` map in the `polkadot_runtime_parachains::paras` pallet.
const PARAS_LIFECYCLES_STORAGE_NAME: &str = "ParaLifecycles";

/// Register parachain.
#[derive(StructOpt, Debug, PartialEq)]
pub struct RegisterParachain {
	/// A parachain to register.
	#[structopt(possible_values = Parachain::VARIANTS, case_insensitive = true)]
	parachain: Parachain,
	/// Parachain deposit.
	#[structopt(long, default_value = "0")]
	deposit: Balance,
	/// Lease begin.
	#[structopt(long, default_value = "0")]
	lease_begin: u32,
	/// Lease end.
	#[structopt(long, default_value = "256")]
	lease_end: u32,
	#[structopt(flatten)]
	relay_connection: RelaychainConnectionParams,
	#[structopt(flatten)]
	relay_sign: RelaychainSigningParams,
	#[structopt(flatten)]
	para_connection: ParachainConnectionParams,
}

/// Parachain to register.
#[derive(Debug, EnumString, EnumVariantNames, PartialEq)]
#[strum(serialize_all = "kebab_case")]
pub enum Parachain {
	RialtoParachain,
}

macro_rules! select_bridge {
	($bridge: expr, $generic: tt) => {
		match $bridge {
			Parachain::RialtoParachain => {
				type Relaychain = relay_rialto_client::Rialto;
				type Parachain = relay_rialto_parachain_client::RialtoParachain;

				use bp_rialto::{PARAS_PALLET_NAME, PARAS_REGISTRAR_PALLET_NAME};

				$generic
			},
		}
	};
}

impl RegisterParachain {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_bridge!(self.parachain, {
			let relay_client = self.relay_connection.to_client::<Relaychain>().await?;
			let relay_sign = self.relay_sign.to_keypair::<Relaychain>()?;
			let para_client = self.para_connection.to_client::<Parachain>().await?;

			// hopefully we're the only actor that is registering parachain right now
			// => read next parachain id
			let para_id_key = bp_runtime::storage_value_final_key(
				PARAS_REGISTRAR_PALLET_NAME.as_bytes(),
				NEXT_FREE_PARA_ID_STORAGE_NAME.as_bytes(),
			);
			let para_id: ParaId = relay_client
				.storage_value(StorageKey(para_id_key.to_vec()), None)
				.await?
				.unwrap_or(polkadot_primitives::v2::LOWEST_PUBLIC_ID)
				.max(polkadot_primitives::v2::LOWEST_PUBLIC_ID);
			log::info!(target: "bridge", "Going to reserve parachain id: {:?}", para_id);

			// step 1: reserve a parachain id
			let relay_genesis_hash = *relay_client.genesis_hash();
			let relay_sudo_account: AccountIdOf<Relaychain> = relay_sign.public().into();
			let reserve_parachain_id_call: CallOf<Relaychain> =
				ParaRegistrarCall::reserve {}.into();
			let reserve_parachain_signer = relay_sign.clone();
			let (spec_version, transaction_version) = relay_client.simple_runtime_version().await?;
			wait_until_transaction_is_finalized::<Relaychain>(
				relay_client
					.submit_and_watch_signed_extrinsic(
						relay_sudo_account.clone(),
						move |_, transaction_nonce| {
							Ok(Bytes(
								Relaychain::sign_transaction(SignParam {
									spec_version,
									transaction_version,
									genesis_hash: relay_genesis_hash,
									signer: reserve_parachain_signer,
									era: relay_substrate_client::TransactionEra::immortal(),
									unsigned: UnsignedTransaction::new(
										reserve_parachain_id_call.into(),
										transaction_nonce,
									),
								})?
								.encode(),
							))
						},
					)
					.await?,
			)
			.await?;
			log::info!(target: "bridge", "Reserved parachain id: {:?}", para_id);

			// step 2: register parathread
			let para_genesis_header = para_client.header_by_number(Zero::zero()).await?;
			let para_code = para_client
				.raw_storage_value(StorageKey(CODE.to_vec()), Some(para_genesis_header.hash()))
				.await?
				.ok_or_else(|| {
					anyhow::format_err!("Cannot fetch validation code of {}", Parachain::NAME)
				})?
				.0;
			log::info!(
				target: "bridge",
				"Going to register parachain {:?}: genesis len = {} code len = {}",
				para_id,
				para_genesis_header.encode().len(),
				para_code.len(),
			);
			let register_parathread_call: CallOf<Relaychain> = ParaRegistrarCall::register {
				id: para_id,
				genesis_head: ParaHeadData(para_genesis_header.encode()),
				validation_code: ParaValidationCode(para_code),
			}
			.into();
			let register_parathread_signer = relay_sign.clone();
			wait_until_transaction_is_finalized::<Relaychain>(
				relay_client
					.submit_and_watch_signed_extrinsic(
						relay_sudo_account.clone(),
						move |_, transaction_nonce| {
							Ok(Bytes(
								Relaychain::sign_transaction(SignParam {
									spec_version,
									transaction_version,
									genesis_hash: relay_genesis_hash,
									signer: register_parathread_signer,
									era: relay_substrate_client::TransactionEra::immortal(),
									unsigned: UnsignedTransaction::new(
										register_parathread_call.into(),
										transaction_nonce,
									),
								})?
								.encode(),
							))
						},
					)
					.await?,
			)
			.await?;
			log::info!(target: "bridge", "Registered parachain: {:?}. Waiting for onboarding", para_id);

			// wait until parathread is onboarded
			let para_state_key = bp_runtime::storage_map_final_key::<Twox64Concat>(
				PARAS_PALLET_NAME,
				PARAS_LIFECYCLES_STORAGE_NAME,
				&para_id.encode(),
			);
			wait_para_state(
				&relay_client,
				&para_state_key.0,
				&[ParaLifecycle::Onboarding, ParaLifecycle::Parathread],
				ParaLifecycle::Parathread,
			)
			.await?;

			// step 3: force parachain leases
			let lease_begin = self.lease_begin;
			let lease_end = self.lease_end;
			let para_deposit = self.deposit.cast().into();
			log::info!(
				target: "bridge",
				"Going to force leases of parachain {:?}: [{}; {}]",
				para_id,
				lease_begin,
				lease_end,
			);
			let force_lease_call: CallOf<Relaychain> = SudoCall::sudo {
				call: Box::new(
					ParaSlotsCall::force_lease {
						para: para_id,
						leaser: relay_sudo_account.clone(),
						amount: para_deposit,
						period_begin: lease_begin,
						period_count: lease_end.saturating_sub(lease_begin).saturating_add(1),
					}
					.into(),
				),
			}
			.into();
			let force_lease_signer = relay_sign.clone();
			relay_client
				.submit_signed_extrinsic(relay_sudo_account.clone(), move |_, transaction_nonce| {
					Ok(Bytes(
						Relaychain::sign_transaction(SignParam {
							spec_version,
							transaction_version,
							genesis_hash: relay_genesis_hash,
							signer: force_lease_signer,
							era: relay_substrate_client::TransactionEra::immortal(),
							unsigned: UnsignedTransaction::new(
								force_lease_call.into(),
								transaction_nonce,
							),
						})?
						.encode(),
					))
				})
				.await?;
			log::info!(target: "bridge", "Registered parachain leases: {:?}. Waiting for onboarding", para_id);

			// wait until parachain is onboarded
			wait_para_state(
				&relay_client,
				&para_state_key.0,
				&[
					ParaLifecycle::Onboarding,
					ParaLifecycle::UpgradingParathread,
					ParaLifecycle::Parathread,
				],
				ParaLifecycle::Parachain,
			)
			.await?;

			Ok(())
		})
	}
}

/// Wait until parachain state is changed.
async fn wait_para_state<Relaychain: Chain>(
	relay_client: &Client<Relaychain>,
	para_state_key: &[u8],
	from_states: &[ParaLifecycle],
	to_state: ParaLifecycle,
) -> anyhow::Result<()> {
	loop {
		let para_state: ParaLifecycle = relay_client
			.storage_value(StorageKey(para_state_key.to_vec()), None)
			.await?
			.ok_or_else(|| {
				anyhow::format_err!(
					"Cannot fetch next free parachain lifecycle from the runtime storage of {}",
					Relaychain::NAME,
				)
			})?;
		if para_state == to_state {
			log::info!(target: "bridge", "Parachain state is now: {:?}", to_state);
			return Ok(())
		}
		if !from_states.contains(&para_state) {
			return Err(anyhow::format_err!("Invalid parachain lifecycle: {:?}", para_state))
		}

		log::info!(target: "bridge", "Parachain state: {:?}. Waiting for {:?}", para_state, to_state);
		async_std::task::sleep(Relaychain::AVERAGE_BLOCK_INTERVAL).await;
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::{
		ParachainRuntimeVersionParams, RelaychainRuntimeVersionParams, RuntimeVersionType,
	};

	#[test]
	fn register_rialto_parachain() {
		let register_parachain = RegisterParachain::from_iter(vec![
			"register-parachain",
			"rialto-parachain",
			"--parachain-host",
			"127.0.0.1",
			"--parachain-port",
			"11949",
			"--relaychain-host",
			"127.0.0.1",
			"--relaychain-port",
			"9944",
			"--relaychain-signer",
			"//Alice",
			"--deposit",
			"42",
			"--lease-begin",
			"100",
			"--lease-end",
			"200",
		]);

		assert_eq!(
			register_parachain,
			RegisterParachain {
				parachain: Parachain::RialtoParachain,
				deposit: Balance(42),
				lease_begin: 100,
				lease_end: 200,
				relay_connection: RelaychainConnectionParams {
					relaychain_host: "127.0.0.1".into(),
					relaychain_port: 9944,
					relaychain_secure: false,
					relaychain_runtime_version: RelaychainRuntimeVersionParams {
						relaychain_version_mode: RuntimeVersionType::Bundle,
						relaychain_spec_version: None,
						relaychain_transaction_version: None,
					}
				},
				relay_sign: RelaychainSigningParams {
					relaychain_signer: Some("//Alice".into()),
					relaychain_signer_password: None,
					relaychain_signer_file: None,
					relaychain_signer_password_file: None,
					relaychain_transactions_mortality: None,
				},
				para_connection: ParachainConnectionParams {
					parachain_host: "127.0.0.1".into(),
					parachain_port: 11949,
					parachain_secure: false,
					parachain_runtime_version: ParachainRuntimeVersionParams {
						parachain_version_mode: RuntimeVersionType::Bundle,
						parachain_spec_version: None,
						parachain_transaction_version: None,
					}
				},
			}
		);
	}
}
